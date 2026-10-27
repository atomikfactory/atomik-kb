//! The WAV file a speech segment becomes for the Ollama engine (ADR-0053): 16 kHz, mono, 16-bit PCM,
//! the format `ai_transcribe` accepts (`kb_ai::transcript::is_pcm_wav`).

use crate::protocol::SAMPLE_RATE_HZ;

/// The bytes of the RIFF header of the file [`encode_wav`] writes.
pub const WAV_HEADER_BYTES: usize = 44;

/// `pcm` (16 kHz mono `i16`) as a PCM WAV file. A segment of at most the hard cap plus the pre-roll is far
/// under the 4 MiB `ai_transcribe` takes.
#[must_use]
pub fn encode_wav(pcm: &[i16]) -> Vec<u8> {
    let data_bytes = u32::try_from(pcm.len().saturating_mul(2)).unwrap_or(u32::MAX - 36);
    let mut wav = Vec::with_capacity(WAV_HEADER_BYTES + pcm.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&data_bytes.saturating_add(36).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE_HZ.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE_HZ * 2).to_le_bytes()); // bytes per second
    wav.extend_from_slice(&2_u16.to_le_bytes()); // bytes per sample frame
    wav.extend_from_slice(&16_u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_header_describes_16_khz_mono_16_bit_pcm() {
        let wav = encode_wav(&[1, -2, 3]);
        assert_eq!(wav.len(), WAV_HEADER_BYTES + 6);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(&wav[44..], &[1, 0, 0xfe, 0xff, 3, 0]);
    }

    #[test]
    fn what_ai_transcribe_accepts_is_what_this_makes() {
        assert!(kb_ai::transcript::is_pcm_wav(&encode_wav(&[0; 16_000])));
        assert!(kb_ai::transcript::is_pcm_wav(&encode_wav(&[])));
    }

    #[test]
    fn a_segment_at_the_longest_cap_is_far_under_the_four_mebibyte_limit() {
        let longest = crate::protocol::MAX_HARD_CAP_MS + crate::protocol::MAX_PREROLL_MS;
        let samples = crate::policy::samples_for_ms(longest);
        assert!(WAV_HEADER_BYTES + samples * 2 < 4 * 1024 * 1024);
    }
}
