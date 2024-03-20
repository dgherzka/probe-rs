//! Sequences for STM32N6 devices

use crate::architecture::arm::armv8m::{Aircr, Dhcsr};
use crate::architecture::arm::memory::adi_v5_memory_interface::ArmProbe;
use crate::architecture::arm::sequences::ArmDebugSequence;
use crate::architecture::arm::ArmError;
use crate::core::memory_mapped_registers::MemoryMappedRegister;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Marker struct indicating initialization sequencing for STM32N6 parts.
#[derive(Debug)]
pub struct Stm32n6 {}

impl Stm32n6 {
    /// Create a sequencer for STM32N6 parts.
    pub fn create() -> Arc<Self> {
        Arc::new(Self {})
    }
}

impl ArmDebugSequence for Stm32n6 {
    fn reset_system(
        &self,
        interface: &mut dyn ArmProbe,
        _core_type: crate::CoreType,
        _debug_base: Option<u64>,
    ) -> Result<(), ArmError> {
        let mut aircr = Aircr(0);
        aircr.vectkey();
        aircr.set_sysresetreq(true);

        interface.write_word_32(Aircr::get_mmio_address(), aircr.into())?;

        let start = Instant::now();

        // After reset, AP1 is locked until the boot ROM opens it.
        //
        // We could poll DBGMCU_SR.AP1_ENABLE (addr 0x8000_10FC bit 17) on AP0
        // to see when it unlocks, but the usual `dyn ArmProbe` above is
        // `StLinkMemoryInterface` which lacks `get_arm_communication_interface()`,
        // so there's no way to navigate to AP0.
        //
        // Instead, just retry all errors for a little while.
        loop {
            if let Ok(dhcsr) = interface.read_word_32(Dhcsr::get_mmio_address()) {
                // Wait until the S_RESET_ST bit is cleared on a read
                if !Dhcsr(dhcsr).s_reset_st() {
                    tracing::info!("reset complete after {:?}", start.elapsed());
                    return Ok(());
                }
            }
            if start.elapsed() > Duration::from_millis(500) {
                tracing::warn!("reset timed out after {:?}", start.elapsed());
                return Err(ArmError::Timeout);
            }
        }
    }
}
