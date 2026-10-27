//! The `kb-sync` v2 conformance suite against the real server, in process, over HTTP, through
//! `kb_net::HttpRemote` (protocol v2) with the server's own clock and pruning pass wired in.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;

use kb_sync::v2::conformance::{self, Fixture};
use kb_types::testing::FixedClock;
use support::{TestServer, config};

/// A fixture whose clock and pruning pass are the server's.
fn fixture(seed: u64, server: &TestServer, clock: &Arc<FixedClock>) -> Fixture {
    let advance = Arc::clone(clock);
    let pruner = server.server.clone();
    Fixture::generate(seed).with_control(
        move |ms| advance.advance(ms),
        move || {
            pruner.prune_now().unwrap();
        },
    )
}

async fn started(
    seed: u64,
    adjust: impl FnOnce(&mut kb_server::Config),
) -> (tempfile::TempDir, TestServer, Arc<FixedClock>, Fixture) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    adjust(&mut config);
    let clock = Arc::new(FixedClock::new(support::T0));
    let server = TestServer::start_with_clock(&config, clock.clone()).await;
    let fixture = fixture(seed, &server, &clock);
    (dir, server, clock, fixture)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_passes_the_v2_conformance_suite() {
    let (_dir, server, _clock, fixture) = started(41, |_| {}).await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::run_all_v2(&remote, &fixture).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_passes_the_v2_quota_check() {
    let (_dir, server, _clock, fixture) =
        started(42, |config| config.account_quota_bytes = 2 * 1024 * 1024).await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::check_quota_passes_to_owner(&remote, &fixture).await;
    server.stop().await;
}

/// Idle-device expiry: the check moves the server's clock.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_passes_the_idle_device_check() {
    let (_dir, server, _clock, fixture) =
        started(48, |config| config.idle_device_days = Some(30)).await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::check_idle_device(&remote, &fixture).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_enforces_the_feed_limit() {
    let (_dir, server, _clock, fixture) = started(43, |_| {}).await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::check_feed_limit(&remote, &fixture).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_enforces_the_total_signup_limit() {
    let (_dir, server, _clock, fixture) = started(44, |config| {
        config.share_signups = 2;
        config.share_signups_per_account = 5;
    })
    .await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::check_share_signup_total_limit(&remote, &fixture).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_enforces_the_per_inviter_signup_limit() {
    let (_dir, server, _clock, fixture) = started(45, |config| {
        config.share_signups = 20;
        config.share_signups_per_account = 2;
    })
    .await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::check_share_signup(&remote, &fixture).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_share_signups_refuses_them() {
    let (_dir, server, _clock, fixture) = started(46, |config| {
        config.share_signups = 0;
    })
    .await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::check_share_signup(&remote, &fixture).await;
    server.stop().await;
}

/// The suite without a way to move the clock skips the expiry, pruning and deletion parts.
#[tokio::test(flavor = "multi_thread")]
async fn the_suite_runs_against_the_server_without_control() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(&config(dir.path())).await;
    let remote = kb_net::HttpRemote::new(&server.base).unwrap();
    conformance::run_all_v2(&remote, &Fixture::generate(47)).await;
    server.stop().await;
}
