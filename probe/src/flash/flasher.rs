use ::memory::MI;
use crate::session::Session;

use super::*;

const ANALYZER: [u32; 49] = [
    0x2780b5f0, 0x25004684, 0x4e2b2401, 0x447e4a2b, 0x0023007f, 0x425b402b, 0x40130868, 0x08584043,
    0x425b4023, 0x40584013, 0x40200843, 0x40104240, 0x08434058, 0x42404020, 0x40584010, 0x40200843,
    0x40104240, 0x08434058, 0x42404020, 0x40584010, 0x40200843, 0x40104240, 0x08584043, 0x425b4023,
    0x40434013, 0xc6083501, 0xd1d242bd, 0xd01f2900, 0x46602301, 0x469c25ff, 0x00894e11, 0x447e1841,
    0x88034667, 0x409f8844, 0x2f00409c, 0x2201d012, 0x4252193f, 0x34017823, 0x402b4053, 0x599b009b,
    0x405a0a12, 0xd1f542bc, 0xc00443d2, 0xd1e74281, 0xbdf02000, 0xe7f82200, 0x000000b2, 0xedb88320,
    0x00000042,
];

#[derive(Debug, Default, Copy, Clone)]
pub struct FlashAlgorithm {
    /// Memory address where the flash algo instructions will be loaded to.
    pub load_address: u32,
    /// List of 32-bit words containing the position-independant code for the algo.
    pub instructions: &'static [u32],
    /// Address of the `Init()` entry point. Optional.
    pub pc_init: Option<u32>,
    /// Address of the `UnInit()` entry point. Optional.
    pub pc_uninit: Option<u32>,
    /// Address of the `ProgramPage()` entry point.
    pub pc_program_page: u32,
    /// Address of the `EraseSector()` entry point.
    pub pc_erase_sector: u32,
    /// Address of the `EraseAll()` entry point. Optional.
    pub pc_erase_all: Option<u32>,
    /// Initial value of the R9 register for calling flash algo entry points, which
    /// determines where the position-independant data resides.
    pub static_base: u32,
    /// Initial value of the stack pointer when calling any flash algo API.
    pub begin_stack: u32,
    /// Base address of the page buffer. Used if `page_buffers` is not provided.
    pub begin_data: u32,
    /// An optional list of base addresses for page buffers. The buffers must be at
    /// least as large as the region's page_size attribute. If at least 2 buffers are included in
    /// the list, then double buffered programming will be enabled.
    pub page_buffers: &'static [u32],
    pub min_program_length: Option<u32>,
    /// Whether the CRC32-based analyzer is supported.
    pub analyzer_supported: bool,
    /// RAM base address where the analyzer code will be placed. There must be at
    /// least 0x600 free bytes after this address.
    pub analyzer_address: u32,
}

pub trait Operation {
    fn operation() -> u32;
}

pub struct Erase;

impl Operation for Erase {
    fn operation() -> u32 { 1 }
}

pub struct Program;

impl Operation for Program {
    fn operation() -> u32 { 2 }
}

pub struct Verify;

impl Operation for Verify {
    fn operation() -> u32 { 3 }
}

pub enum FlasherError {
    Init(u32),
    Uninit(u32),
    EraseAll(u32),
    EraseAllNotSupported,
    EraseSector(u32, u32),
    ProgramPage(u32, u32),
    InvalidBufferNumber(u32, u32),
    UnalignedFlashWriteAddress,
    UnalignedPhraseLength,
    ProgramPhrase(u32, u32),
}

pub struct InactiveFlasher<'a> {
    session: &'a mut Session,
}

impl<'a> InactiveFlasher<'a> {
    pub fn init<O: Operation>(&mut self, region: FlashRegion, address: Option<u32>, clock: Option<u32>) -> Result<ActiveFlasher<O>, FlasherError> {
        let algo = self.session.target.info.flash_algorithm;
        let regs = self.session.target.info.basic_register_addresses;

        // TODO: Halt & reset target.

        // TODO: Possible special preparation of the target such as enabling faster clocks for the flash e.g.

        // Load flash algorithm code into target RAM.
        self.session.probe.write_block32(algo.load_address, algo.instructions);

        let mut flasher = ActiveFlasher {
            session: self.session,
            region,
            _operation: core::marker::PhantomData,
        };

        // Execute init routine if one is present.
        if let Some(pc_init) = algo.pc_init {
            let result = flasher.call_function_and_wait(
                pc_init,
                address,
                clock,
                Some(O::operation()),
                None,
                true
            );

            if result != 0 {
                return Err(FlasherError::Init(result));
            }
        }

        Ok(flasher)
    }
}

pub struct ActiveFlasher<'a, O: Operation> {
    session: &'a mut Session,
    region: FlashRegion,
    _operation: core::marker::PhantomData<O>,
}

impl<'a, O: Operation> ActiveFlasher<'a, O> {
    pub fn uninit(&mut self) -> Result<InactiveFlasher, FlasherError> {
        let algo = self.session.target.info.flash_algorithm;

        if let Some(pc_uninit) = algo.pc_uninit {
            let result = self.call_function_and_wait(
                pc_uninit,
                Some(O::operation()),
                None,
                None,
                None,
                false
            );

            if result != 0 {
                return Err(FlasherError::Uninit(result));
            }
        }

        Ok(InactiveFlasher {
            session: self.session,
        })
    }

    fn call_function_and_wait(&mut self, pc: u32, r0: Option<u32>, r1: Option<u32>, r2: Option<u32>, r3: Option<u32>, init: bool) -> u32 {
        self.call_function(pc, r0, r1, r2, r3, init);
        self.wait_for_completion()
    }

    fn call_function(&mut self, pc: u32, r0: Option<u32>, r1: Option<u32>, r2: Option<u32>, r3: Option<u32>, init: bool) {
        let algo = self.session.target.info.flash_algorithm;
        let regs = self.session.target.info.basic_register_addresses;
        [
            (regs.PC, Some(pc)),
            (regs.R0, r0),
            (regs.R1, r1),
            (regs.R2, r2),
            (regs.R3, r3),
            (regs.R9, if init { Some(algo.static_base) } else { None }),
            (regs.SP, if init { Some(algo.begin_stack) } else { None }),
            (regs.LR, Some(algo.load_address + 1)),
        ].into_iter().for_each(|(addr, value)| if let Some(v) = value {
            self.session.target.core.write_core_reg(&mut self.session.probe, *addr, *v);
        });

        // Resume target operation.
        self.session.target.core.run(&mut self.session.probe);
    }

    fn wait_for_completion(&mut self) -> u32 {
        let regs = self.session.target.info.basic_register_addresses;

        while self.session.target.core.wait_for_core_halted(&mut self.session.probe).is_err() {}

        self.session.target.core.read_core_reg(&mut self.session.probe, regs.R0).unwrap()
    }
}

impl <'a> ActiveFlasher<'a, Erase> {
    pub fn erase_all(&mut self) -> Result<(), FlasherError> {
        let algo = self.session.target.info.flash_algorithm;

        if let Some(pc_erase_all) = algo.pc_erase_all {
            let result = self.call_function_and_wait(
                pc_erase_all,
                None,
                None,
                None,
                None,
                false
            );

            if result != 0 {
                Err(FlasherError::EraseAll(result))
            } else {
                Ok(())
            }
        } else {
            Err(FlasherError::EraseAllNotSupported)
        }
    }

    pub fn erase_sector(&mut self, address: u32) -> Result<(), FlasherError> {
        let algo = self.session.target.info.flash_algorithm;

        let result = self.call_function_and_wait(
            algo.pc_erase_sector,
            Some(address),
            None,
            None,
            None,
            false
        );

        if result != 0 {
            Err(FlasherError::EraseSector(result, address))
        } else {
            Ok(())
        }
    }
}

impl <'a> ActiveFlasher<'a, Program> {
    pub fn program_page(&mut self, address: u32, bytes: &[u8]) -> Result<(), FlasherError> {
        let algo = self.session.target.info.flash_algorithm;

        // TODO: Prevent security settings from locking the device.

        // Transfer the bytes to RAM.
        self.session.probe.write_block8(algo.begin_data, bytes);

        let result = self.call_function_and_wait(
            algo.pc_program_page,
            Some(address),
            Some(bytes.len() as u32),
            Some(algo.begin_data),
            None,
            false
        );

        if result != 0 {
            Err(FlasherError::ProgramPage(result, address))
        } else {
            Ok(())
        }
    }

    pub fn start_program_page_with_buffer(&mut self, address: u32, buffer_number: u32) -> Result<(), FlasherError> {
        let algo = self.session.target.info.flash_algorithm;

        // Check the buffer number.
        if buffer_number < algo.page_buffers.len() as u32 {
            return Err(FlasherError::InvalidBufferNumber(buffer_number, algo.page_buffers.len() as u32));
        }

        self.call_function(
            algo.pc_program_page,
            Some(address),
            Some(self.region.page_size),
            Some(algo.page_buffers[buffer_number as usize]),
            None,
            false
        );

        Ok(())
    }

    pub fn load_page_buffer(&mut self, address: u32, bytes: &[u8], buffer_number: u32) -> Result<(), FlasherError> {
        let algo = self.session.target.info.flash_algorithm;

        // Check the buffer number.
        if buffer_number < algo.page_buffers.len() as u32 {
            return Err(FlasherError::InvalidBufferNumber(buffer_number, algo.page_buffers.len() as u32));
        }

        // TODO: Prevent security settings from locking the device.

        // Transfer the buffer bytes to RAM.
        self.session.probe.write_block8(algo.page_buffers[buffer_number as usize], bytes);

        Ok(())
    }

    pub fn program_phrase(&mut self, address: u32, bytes: &[u8]) -> Result<(), FlasherError> {
        let algo = self.session.target.info.flash_algorithm;

        // Get the minimum programming length. If none was specified, use the page size.
        let min_len = if let Some(min_program_length) = algo.min_program_length {
            min_program_length
        } else {
            self.region.page_size
        };

        // Require write address and length to be aligned to the minimum write size.
        if address % min_len != 0 {
            return Err(FlasherError::UnalignedFlashWriteAddress);
        }
        if bytes.len() as u32 % min_len != 0 {
            return Err(FlasherError::UnalignedPhraseLength);
        }

        // TODO: Prevent security settings from locking the device.

        // Transfer the phrase bytes to RAM.
        self.session.probe.write_block8(algo.begin_data, bytes);

        let result = self.call_function_and_wait(
            algo.pc_program_page,
            Some(address),
            Some(bytes.len() as u32),
            Some(algo.begin_data),
            None,
            false
        );

        if result != 0 {
            Err(FlasherError::ProgramPhrase(result, address))
        } else {
            Ok(())
        }
    }

    pub fn get_sector_info(&self, address: u32) -> Option<SectorInfo> {
        if !self.region.range.contains(&address) {
            return None
        }

        Some(SectorInfo {
            base_address: address - (address % self.region.sector_size),
            erase_weight: self.region.erase_sector_weight,
            size: self.region.sector_size,
        })
    }

    pub fn get_page_info(&self, address: u32) -> Option<PageInfo> {
        if !self.region.range.contains(&address) {
            return None
        }

        Some(PageInfo {
            base_address: address - (address % self.region.page_size),
            program_weight: self.region.program_page_weight,
            size: self.region.page_size,
        })
    }

    pub fn get_flash_info(&self, address: u32) -> Option<FlashInfo> {
        if !self.region.range.contains(&address) {
            return None
        }

        let algo = self.session.target.info.flash_algorithm;

        Some(FlashInfo {
            rom_start: self.region.range.start,
            erase_weight: self.region.erase_all_weight,
            crc_supported: algo.analyzer_supported,
        })
    }
}