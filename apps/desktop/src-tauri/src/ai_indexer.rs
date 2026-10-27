//! The semantic index's background worker (ADR-0037, `docs/architecture/ai.md`, section 6.3): a
//! background task like the sync driver. It runs while the core is ready, finds the pages whose vectors
//! are stale (`AiIndexNext`), has Ollama embed their chunks, and stores the vectors (`AiIndexStore`).
//!
//! **When it runs.** At start; five seconds after the last `PageContentChanged`, `StructureChanged` or
//! `SettingChanged` (`app.ai.settings`, `app.ai.endpoint`) event (a person typing keeps it waiting, and
//! a batch in progress ends at its next boundary when one arrives); after an `AiIndexChanged` that is not
//! its own (the index was cleared: Settings → Rebuild), which it ignores while it runs; and every 60 seconds. It asks the
//! core first: while AI or the index is off, or no embedding model is chosen, the core answers with no
//! model and the worker is idle, not failing.
//!
//! **When it fails.** An unreachable Ollama, a model that is not installed, an endpoint without consent,
//! a core that answers an error: the state becomes `error` with a short sentence (never page text and
//! never the host), and the worker tries again after 60 s, then 2, 4, 8 minutes and 10 minutes at most.
//! Content events do not shorten that wait; a change of the AI settings or of the endpoint ends it.
//!
//! **A batch** is `AiIndexNext { maxChunks: 16 }`; the chunks that need a vector are embedded 16 at a
//! time with the model's document prefix (`kb_ai::chunk::embedding_input`), then the page is stored; a
//! pause of 200 ms follows, and the loop repeats until the core answers with no pages (a batch that
//! comes back unchanged ends the run: those pages did not become clean). The core lock is never held while
//! Ollama is used. Dropping the task (the core locked) stops it, cancels every AI stream and resets the
//! state it reported.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use kb_ai::chunk::{EmbedRole, embedding_input};
use kb_types::ai::{
    AI_ENDPOINT_SETTING, AI_SETTINGS_SETTING, AiChunkVector, AiIndexPage, AiIndexerPhase,
};
use kb_types::{ApiError, Clock, Event, PageId, Request, Response};
use tokio::sync::Notify;

use crate::ai_client::reason_of;
use crate::ai_host::{AiShell, allowed_url, core, read_endpoint};
use crate::state::{BackgroundStarter, BackgroundTask, CoreHost};

/// How many chunks are embedded in one call to Ollama.
pub const EMBED_BATCH: usize = 16;

/// `maxChunks` of each `AiIndexNext`.
const MAX_CHUNKS: u32 = 16;

/// When the worker looks.
#[derive(Debug, Clone, Copy)]
pub struct IndexerTiming {
    /// How long the events must have stopped before it looks (5 s).
    pub debounce: Duration,
    /// How often it looks without an event (60 s).
    pub tick: Duration,
    /// The wait after the first failure (60 s); it doubles with each one.
    pub backoff_first: Duration,
    /// The longest wait after a failure (10 min).
    pub backoff_max: Duration,
    /// The pause between two batches (200 ms).
    pub pause: Duration,
}

impl IndexerTiming {
    /// 5 s, 60 s, 60 s, 10 min and 200 ms.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            debounce: Duration::from_secs(5),
            tick: Duration::from_secs(60),
            backoff_first: Duration::from_secs(60),
            backoff_max: Duration::from_secs(10 * 60),
            pause: Duration::from_millis(200),
        }
    }

    /// The wait after the `failures`-th failure in a row (1 for the first).
    fn backoff(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        self.backoff_first
            .saturating_mul(1_u32 << doublings)
            .min(self.backoff_max)
    }
}

/// What the event listener tells the worker.
#[derive(Default)]
struct Wake {
    notify: Notify,
    /// The AI settings or the endpoint changed.
    settings: AtomicBool,
    /// A page or the structure changed.
    content: AtomicBool,
    /// The worker is in a run: the `AiIndexChanged` events it hears are those of its own
    /// `AiIndexNext` and `AiIndexStore` (they are emitted under the core's lock, before the call
    /// returns), and must not wake it again.
    running: AtomicBool,
}

impl Wake {
    /// Called under the core's lock for every event: it only sets a flag and wakes the worker.
    fn hear(&self, event: &Event) {
        match event {
            Event::PageContentChanged { .. } | Event::StructureChanged { .. } => {
                self.content.store(true, Ordering::SeqCst);
                self.notify.notify_one();
            }
            Event::SettingChanged { key }
                if key == AI_SETTINGS_SETTING || key == AI_ENDPOINT_SETTING =>
            {
                self.settings.store(true, Ordering::SeqCst);
                self.notify.notify_one();
            }
            // The index changed behind the worker's back (Settings → Rebuild clears it): look after
            // the debounce, instead of at the next tick. Not while it indexes itself.
            Event::AiIndexChanged if !self.running.load(Ordering::SeqCst) => {
                self.notify.notify_one();
            }
            _ => {}
        }
    }
}

/// The running worker. Dropping it stops the task, takes its listener off the state, cancels the AI
/// streams (they belong to a workspace that is no longer open) and resets the reported state.
struct IndexerTask {
    handle: tauri::async_runtime::JoinHandle<()>,
    unlisten: Option<Box<dyn FnOnce() + Send>>,
    shell: Arc<AiShell>,
}

impl Drop for IndexerTask {
    fn drop(&mut self) {
        self.handle.abort();
        if let Some(unlisten) = self.unlisten.take() {
            unlisten();
        }
        self.shell.cancel_all();
        self.shell.reset_indexer();
    }
}

/// The background work `lib.rs` registers next to the sync driver: starts the worker when the core is
/// ready and stops it, and the AI streams, when it locks. It holds the state only weakly: when the
/// state is gone the task ends.
pub fn indexer_starter(
    host: Weak<dyn CoreHost>,
    shell: Arc<AiShell>,
    clock: Arc<dyn Clock>,
    timing: IndexerTiming,
) -> BackgroundStarter {
    Box::new(move || {
        let strong = host.upgrade()?;
        let wake = Arc::new(Wake::default());
        let listener = Arc::clone(&wake);
        let unlisten = strong.add_listener(Arc::new(move |event: &Event| listener.hear(event)));
        drop(strong);
        let handle = tauri::async_runtime::spawn(run(
            host.clone(),
            Arc::clone(&shell),
            Arc::clone(&clock),
            timing,
            wake,
        ));
        let task: BackgroundTask = Box::new(IndexerTask {
            handle,
            unlisten: Some(unlisten),
            shell: Arc::clone(&shell),
        });
        Some(task)
    })
}

/// The loop: look, wait for a reason to look again, look.
async fn run(
    host: Weak<dyn CoreHost>,
    shell: Arc<AiShell>,
    clock: Arc<dyn Clock>,
    timing: IndexerTiming,
    wake: Arc<Wake>,
) {
    let mut failures = 0_u32;
    let mut retry_at: Option<Instant> = None;
    let mut first = true;
    loop {
        if !first {
            wait(&wake, &timing, retry_at).await;
        }
        first = false;
        if wake.settings.swap(false, Ordering::SeqCst) {
            failures = 0;
            retry_at = None;
        }
        if retry_at.is_some_and(|at| Instant::now() < at) {
            continue;
        }
        let Some(strong) = host.upgrade() else {
            return;
        };
        wake.content.store(false, Ordering::SeqCst);
        wake.running.store(true, Ordering::SeqCst);
        let outcome = index_pending(&strong, &shell, &*clock, &timing, &wake).await;
        wake.running.store(false, Ordering::SeqCst);
        match outcome {
            Ok(()) => {
                failures = 0;
                retry_at = None;
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                let delay = timing.backoff(failures);
                retry_at = Some(Instant::now() + delay);
                shell.update_indexer(|state| {
                    state.phase = AiIndexerPhase::Error;
                    state.error = Some(error.message.clone());
                });
                tracing::info!(
                    code = ?error.code,
                    reason = reason_of(&error),
                    failures,
                    "the semantic index could not be updated"
                );
            }
        }
    }
}

/// Waits for a reason to look: the tick (or the end of the wait after a failure, when that comes
/// first), or an event followed by a quiet debounce.
async fn wait(wake: &Wake, timing: &IndexerTiming, retry_at: Option<Instant>) {
    let nap = retry_at.map_or(timing.tick, |at| {
        at.saturating_duration_since(Instant::now())
    });
    tokio::select! {
        () = tokio::time::sleep(nap) => {}
        () = wake.notify.notified() => {
            loop {
                tokio::select! {
                    () = wake.notify.notified() => {}
                    () = tokio::time::sleep(timing.debounce) => break,
                }
            }
        }
    }
}

/// Indexes every page that is stale, a batch at a time, until none is left, or an event says the
/// content changed again (the next look comes after the debounce).
async fn index_pending(
    host: &Arc<dyn CoreHost>,
    shell: &AiShell,
    clock: &dyn Clock,
    timing: &IndexerTiming,
    wake: &Wake,
) -> Result<(), ApiError> {
    // The pages (and their hashes) of the batch before: a batch that comes back unchanged means the
    // pages did not become clean, and asking again at once would only spin.
    let mut previous: Option<Vec<(PageId, String)>> = None;
    loop {
        let work = core(host, |host| {
            match host.dispatch(Request::AiIndexNext {
                max_chunks: MAX_CHUNKS,
            })? {
                Response::AiIndexWork(work) => Ok(work),
                _ => Err(ApiError::internal(
                    "the core answered with something unexpected",
                )),
            }
        })
        .await?;
        let Some(model) = work.model.clone() else {
            settle(shell, clock, false);
            return Ok(());
        };
        if work.pages.is_empty() {
            settle(shell, clock, false);
            return Ok(());
        }
        let signature: Vec<(PageId, String)> = work
            .pages
            .iter()
            .map(|page| (page.page_id, page.source_hash.clone()))
            .collect();
        if previous.as_ref() == Some(&signature) {
            settle(shell, clock, false);
            return Ok(());
        }
        shell.update_indexer(|state| {
            state.phase = AiIndexerPhase::Indexing;
            state.error = None;
        });
        let endpoint = core(host, |host| allowed_url(&read_endpoint(host)?)).await?;
        let mut stored = 0_usize;
        for page in work.pages {
            index_page(host, shell, &endpoint, &model, page).await?;
            stored += 1;
        }
        tracing::debug!(
            pages = stored,
            remaining = work.remaining,
            "semantic index batch stored"
        );
        settle(shell, clock, true);
        previous = Some(signature);
        // Asked again whether `remaining` says so or not: a store that was stale (the page changed
        // under it) leaves the page dirty without counting it, and the next answer is the proof that
        // nothing is left (no pages).
        if wake.content.load(Ordering::SeqCst) {
            // The person is editing: leave the rest to the next look, after the debounce.
            return Ok(());
        }
        tokio::time::sleep(timing.pause).await;
    }
}

/// The worker has nothing to do (for now): idle, and the time of the last stored batch when one was.
fn settle(shell: &AiShell, clock: &dyn Clock, stored: bool) {
    let now = clock.now_ms();
    shell.update_indexer(|state| {
        state.phase = AiIndexerPhase::Idle;
        state.error = None;
        if stored {
            state.last_run_at = Some(now);
        }
    });
}

/// Embeds the chunks of one page that need a vector and stores the page.
async fn index_page(
    host: &Arc<dyn CoreHost>,
    shell: &AiShell,
    endpoint: &str,
    model: &str,
    page: AiIndexPage,
) -> Result<(), ApiError> {
    let wanted: Vec<_> = page
        .chunks
        .iter()
        .filter(|chunk| chunk.needs_vector)
        .collect();
    let mut vectors = Vec::with_capacity(wanted.len());
    for batch in wanted.chunks(EMBED_BATCH) {
        let inputs: Vec<String> = batch
            .iter()
            .map(|chunk| embedding_input(model, &chunk.text, EmbedRole::Document))
            .collect();
        let embedded = shell.client.embed(endpoint, model, inputs, None).await?;
        for (chunk, vector) in batch.iter().zip(embedded) {
            if vector.is_empty() {
                return Err(ApiError::internal("Ollama answered an empty embedding"));
            }
            vectors.push(AiChunkVector {
                seq: chunk.seq,
                vector,
            });
        }
    }
    let store = Request::AiIndexStore {
        page_id: page.page_id,
        source_hash: page.source_hash,
        model: model.to_owned(),
        vectors,
    };
    core(host, move |host| match host.dispatch(store)? {
        Response::Ok => Ok(()),
        _ => Err(ApiError::internal(
            "the core answered with something unexpected",
        )),
    })
    .await
}

#[cfg(test)]
mod tests;
