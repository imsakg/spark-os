//! The `kernel` binary.
//!
//! # Code organization and architecture
//!
//! The code is divided into different *modules*, each representing a typical **subsystem** of the
//! `kernel`. Top-level module files of subsystems reside directly in the `src` folder. For example,
//! `src/memory.rs` contains code that is concerned with all things memory management.
//!
//! ## Visibility of processor architecture code
//!
//! Some of the `kernel`'s subsystems depend on low-level code that is specific to the target
//! processor architecture. For each supported processor architecture, there exists a subfolder in
//! `src/_arch`, for example, `src/_arch/aarch64`.
//!
//! The architecture folders mirror the subsystem modules laid out in `src`. For example,
//! architectural code that belongs to the `kernel`'s MMU subsystem (`src/memory/mmu.rs`) would go
//! into `src/_arch/aarch64/memory/mmu.rs`. The latter file is loaded as a module in
//! `src/memory/mmu.rs` using the `path attribute`. Usually, the chosen module name is the generic
//! module's name prefixed with `arch_`.
//!
//! For example, this is the top of `src/memory/mmu.rs`:
//!
//! ```
//! #[cfg(target_arch = "aarch64")]
//! #[path = "../_arch/aarch64/memory/mmu.rs"]
//! mod arch_mmu;
//! ```
//!
//! Often times, items from the `arch_ module` will be publicly reexported by the parent module.
//! This way, each architecture specific module can provide its implementation of an item, while the
//! caller must not be concerned which architecture has been conditionally compiled.
//!
//! ## BSP code
//!
//! `BSP` stands for Board Support Package. `BSP` code is organized under `src/bsp.rs` and contains
//! target board specific definitions and functions. These are things such as the board's memory map
//! or instances of drivers for devices that are featured on the respective board.
//!
//! Just like processor architecture code, the `BSP` code's module structure tries to mirror the
//! `kernel`'s subsystem modules, but there is no reexporting this time. That means whatever is
//! provided must be called starting from the `bsp` namespace, e.g. `bsp::driver::driver_manager()`.
//!
//! ## Kernel interfaces
//!
//! Both `arch` and `bsp` contain code that is conditionally compiled depending on the actual target
//! and board for which the kernel is compiled. For example, the `interrupt controller` hardware of
//! the `Raspberry Pi 3` and the `Raspberry Pi 4` is different, but we want the rest of the `kernel`
//! code to play nicely with any of the two without much hassle.
//!
//! In order to provide a clean abstraction between `arch`, `bsp` and `generic kernel code`,
//! `interface` traits are provided *whenever possible* and *where it makes sense*. They are defined
//! in the respective subsystem module and help to enforce the idiom of *program to an interface,
//! not an implementation*. For example, there will be a common IRQ handling interface which the two
//! different interrupt controller `drivers` of both Raspberrys will implement, and only export the
//! interface to the rest of the `kernel`.
//!
//! ```
//!         +-------------------+
//!         | Interface (Trait) |
//!         |                   |
//!         +--+-------------+--+
//!            ^             ^
//!            |             |
//!            |             |
//! +----------+--+       +--+----------+
//! | kernel code |       |  bsp code   |
//! |             |       |  arch code  |
//! +-------------+       +-------------+
//! ```
//!
//! # Summary
//!
//! For a logical `kernel` subsystem, corresponding code can be distributed over several physical
//! locations. Here is an example for the **memory** subsystem:
//!
//! - `src/memory.rs` and `src/memory/**/*`
//!   - Common code that is agnostic of target processor architecture and `BSP` characteristics.
//!     - Example: A function to zero a chunk of memory.
//!   - Interfaces for the memory subsystem that are implemented by `arch` or `BSP` code.
//!     - Example: An `MMU` interface that defines `MMU` function prototypes.
//! - `src/bsp/__board_name__/memory.rs` and `src/bsp/__board_name__/memory/**/*`
//!   - `BSP` specific code.
//!   - Example: The board's memory map (physical addresses of DRAM and MMIO devices).
//! - `src/_arch/__arch_name__/memory.rs` and `src/_arch/__arch_name__/memory/**/*`
//!   - Processor architecture specific code.
//!   - Example: Implementation of the `MMU` interface for the `__arch_name__` processor
//!     architecture.
//!
//! From a namespace perspective, **memory** subsystem code lives in:
//!
//! - `crate::memory::*`
//! - `crate::bsp::memory::*`
//!
//! # Boot flow
//!
//! 1. The kernel's entry point is the function `cpu::boot::arch_boot::_start()`.
//!     - It is implemented in `src/_arch/__arch_name__/cpu/boot.s`.
//! 2. Once finished with architectural setup, the arch code calls `kernel_init()`.

#![feature(asm_const)]
#![feature(format_args_nl)]
#![feature(panic_info_message)]
#![feature(trait_alias)]
#![no_main]
#![no_std]

mod bsp;
mod console;
mod cpu;
mod panic_wait;
mod print;
mod synchronization;

/// Early init code.
///
/// # Safety
///
/// - Only a single core must be active and running this function.
unsafe fn kernel_init() -> ! {
    use console::console;

    // QEMU serial does not model the firmware clock tree; physical boards must
    // configure this UART clock and GPIO routing before using the same adapter.
    bsp::console::initialize(48_000_000).expect("polling UART initialization");
    println!("[0] Hello from Rust!");

    println!("[1] Chars written: {}", console().chars_written());

    let report = spark_os::boot::run_diagnostics().expect("kernel service diagnostics");
    println!("[2] kernel services: {} suites passed", report.suites);

    let mut scheduler = spark_os::process::Scheduler::<1>::new();
    scheduler.spawn().expect("boot caller");
    scheduler.dispatch();
    let mut serial = bsp::console::SyscallConsole;
    let mut message = *b"[3] syscall UART: ok\r\n";
    let length = message.len() as u64;
    let mut memory = spark_os::syscall::UserMemory::new(0x1000, &mut message).expect("boot buffer");
    let result = spark_os::syscall::Services::new(&mut scheduler, &mut serial).dispatch(
        spark_os::syscall::Request {
            number: 64,
            arguments: [1, 0x1000, length, 0, 0, 0],
        },
        &mut memory,
    );
    assert_eq!(result, Ok(length));
    println!("[4] Stopping here.");
    cpu::wait_forever()
}
