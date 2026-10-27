//! Test identities shared by the unit tests of this crate.

#![allow(clippy::unwrap_used)]

use kb_keys::{DeviceKeys, DeviceLog, RecoveryKey};
use kb_types::DeviceId;
use kb_types::testing::SeededRng;

/// A deterministic device.
pub(crate) struct TestDevice {
    pub(crate) id: DeviceId,
    pub(crate) keys: DeviceKeys,
}

impl TestDevice {
    pub(crate) fn new(seed: u64) -> Self {
        let rng = SeededRng::new(seed);
        Self {
            id: DeviceId::generate(&rng),
            keys: DeviceKeys::generate(&rng),
        }
    }
}

/// A one-device log for a fresh account.
pub(crate) fn genesis_log(seed: u64) -> (TestDevice, DeviceLog) {
    let device = TestDevice::new(seed);
    let recovery = RecoveryKey::generate(&SeededRng::new(seed.wrapping_add(1000)));
    let log = DeviceLog::genesis(
        device.id,
        &device.keys,
        recovery.verifying_key(),
        vec![0xAA; 24],
        1,
    )
    .unwrap();
    (device, log)
}
