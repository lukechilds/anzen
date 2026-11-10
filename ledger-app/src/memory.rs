//! Memory measurements for development. The stack and the heap share 36 KB, so both are worth
//! knowing exactly, and the emulator and a real device do not use them identically.
//!
//! `--features measure` paints the unused stack at start and answers the `MEMORY` command with
//! the high-water marks, which works on a device. `--features debug` also prints them to the
//! emulator's log at checkpoints (semihosting: never load a `debug` build on a device).

#[cfg(feature = "measure")]
mod measure {
    use alloc::vec::Vec;
    use core::arch::asm;

    unsafe extern "C" {
        static _stack: u8;
        static _estack: u8;
    }

    const PAINT: u8 = 0xa5;

    fn stack_pointer() -> usize {
        let pointer: usize;
        unsafe { asm!("mov {}, sp", out(reg) pointer) };
        pointer
    }

    fn bounds() -> (usize, usize) {
        (&raw const _stack as usize, &raw const _estack as usize)
    }

    /// Fill the unused stack with a pattern, so later reports can see how deep it went.
    pub fn paint() {
        let (bottom, _) = bounds();
        let top = stack_pointer() - 128;
        for address in bottom..top {
            unsafe { core::ptr::write_volatile(address as *mut u8, PAINT) };
        }
    }

    /// The largest block the heap can still give.
    fn largest_free_block() -> usize {
        let (mut low, mut high) = (0_usize, 40_000_usize);
        while high - low > 64 {
            let middle = (low + high) / 2;
            let mut probe = Vec::<u8>::new();
            if probe.try_reserve_exact(middle).is_ok() {
                low = middle;
            } else {
                high = middle;
            }
        }
        low
    }

    /// The stack's size, its deepest use since the last paint, its use now, and the heap's
    /// largest free block.
    pub fn usage() -> [usize; 4] {
        let (bottom, top) = bounds();
        let mut deepest = bottom;
        while deepest < top && unsafe { core::ptr::read_volatile(deepest as *const u8) } == PAINT {
            deepest += 1;
        }
        [
            top - bottom,
            top - deepest,
            top - stack_pointer(),
            largest_free_block(),
        ]
    }
}

#[cfg(feature = "measure")]
pub use measure::{paint, usage};

#[cfg(not(feature = "measure"))]
pub fn paint() {}

/// Print the marks to the emulator's log.
#[cfg(feature = "debug")]
pub fn report(label: &str) {
    let [size, deepest, now, free] = usage();
    #[allow(deprecated)]
    ledger_device_sdk::testing::debug_print(&alloc::format!(
        "memory {label}: stack {deepest} of {size} used at its deepest, {now} now; heap's largest free block {free}\n",
    ));
}
#[cfg(not(feature = "debug"))]
pub fn report(_label: &str) {}

/// Print a panic's location. No allocation: the heap may be what failed.
#[cfg(feature = "debug")]
pub fn note(info: &core::panic::PanicInfo) {
    #[allow(deprecated)]
    use ledger_device_sdk::testing::debug_print;
    debug_print("panic at ");
    if let Some(location) = info.location() {
        debug_print(location.file());
        let mut digits = [0_u8; 10];
        let (mut line, mut start) = (location.line(), digits.len());
        while line > 0 {
            start -= 1;
            digits[start] = b'0' + (line % 10) as u8;
            line /= 10;
        }
        debug_print(":");
        debug_print(core::str::from_utf8(&digits[start..]).unwrap_or("?"));
    }
    debug_print("\n");
}
#[cfg(not(feature = "debug"))]
pub fn note(_info: &core::panic::PanicInfo) {}
