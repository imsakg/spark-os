# Register-transport peripheral drivers

All production code under `src/drivers/` is `no_std`, dependency-free, and uses
fixed inline storage. Drivers own a `RegisterIo` transport instead of hardcoding
physical addresses. The library does **not** install these drivers into the
boot console, configure the MMU, install exception vectors, unmask CPU interrupts,
or establish user/EL0 execution. Board integration must explicitly do that work.

## MMIO safety boundary

`RegisterIo::{read32, write32}` use **byte offsets** and full 32-bit accesses.
`unsafe VolatileMmio::from_raw(base: *mut u8, bytes: usize)` checks null,
alignment, span length, address overflow, and per-access bounds. It cannot prove
that an address is mapped, is the right peripheral, or is safe to access.
The caller must exclusively own a live mapping for its full lifetime and select
correct device-memory attributes, privilege/access policy, and architecture
barriers. `VolatileMmio` intentionally has no `Send`/`Sync` implementation.
Invalid offsets panic before pointer dereference, rather than causing unchecked
out-of-range MMIO. Do not clone overlapping mappings or alias accesses.

Typical CPU physical peripheral bases are `0x3f000000` on RPi3 and `0xfe000000`
on RPi4; these are **not** firmware bus addresses and are not automatically valid
virtual mappings. Select/translate through the BSP before constructing a transport.
The relevant offsets and minimum spans are:

- PL011: peripheral offset `0x201000`, span `0x48`.
- GPIO: offset `0x200000`, span `0xa0` for legacy pulls or `0xf4` for BCM2711.
- System timer: offset `0x3000`, span `0x1c`.
- Legacy peripheral IRQ controller: offset `0xb200`, span `0x28`, RPi3 only here.
- Mailbox: offset `0xb880`, span `0x3c` (includes MAIL1_STATUS).

## Real console integration: PL011

Use `drivers::pl011::{Pl011, BaudDivisors, UartError}` with a transport rooted at
the actual UART register block. The integration sequence is:

1. Select the board's mapped UART base and provide exclusive access.
2. Establish the real UART input clock (not the CPU/core clock).
3. Route GPIO14/15 to `Function::Alt0`; configure their pulls with the correct
   SoC layout and a board-qualified delay for legacy GPPUD handshaking.
4. Construct `Pl011::new(mmio)`; call
   `initialize(actual_clock_hz, baud, status_read_budget)`.
5. Adapt `write_byte(byte, budget)` to the existing console synchronization and
   formatting traits. Iterate `str::bytes()` for UTF-8; the driver does no
   character truncation, newline conversion, buffering, or implicit formatting.
6. Use `read_byte(budget)` for RX and `flush(budget)` before teardown when needed.

The minimum console adapter operations are therefore:

```rust
let mmio = unsafe { VolatileMmio::from_raw(mapped_uart_base, 0x48)? };
let mut uart = Pl011::new(mmio);
uart.initialize(actual_uart_clock_hz, 115_200, 10_000)?;
uart.write_byte(b'S', 10_000)?;
uart.flush(10_000)?;
```

This is an integration fragment: the board must handle `MmioError` and
`UartError` according to boot policy, not unwrap arbitrary device failures.
`release()` returns transport ownership without changing hardware.

TX polls UARTFR.TXFF before writing UARTDR. RX polls RXFE; erroneous bytes return
`Receive(bits)` with framing/parity/break/overrun in bits 0/1/2/3, and UARTECR is
cleared. Poll budgets count status reads, not elapsed time; zero does no I/O.
`BaudDivisors::calculate` uses widened arithmetic and nearest-1/64 rounding,
including fractional carry, while rejecting zero and out-of-range divisors.
Initialization disables CR, waits for BUSY to clear, masks UART interrupts,
clears pending/error state, programs IBRD/FBRD and FIFO 8-N-1, then enables RX/TX.
Invalid baud configuration makes no accesses; busy timeout leaves CR disabled.

## Other implemented register operations

- **ByteRing<N>:** exactly `N` usable bytes, FIFO wrap, explicit full rejection,
  partial slice I/O, and defined zero-capacity behavior. It is serialized storage,
  not a lock-free IRQ/SMP queue.
- **GPIO:** checked SoC pins (not header pin numbers); GPFSEL RMW preserves
  neighboring functions; GPSET/GPCLR use direct write-one masks; GPLEV samples
  banked levels. Legacy BCM2835/2836/2837 exposes 54 pins and GPPUD/GPPUDCLK.
  BCM2711 exposes 58 pins and two-bit GPPUPPDN fields with different pull encoding.
  Legacy delay callbacks must conservatively satisfy the datasheet's 150-cycle
  requirement, whose clock is unspecified: an empty loop is not a qualified
  delay. GPIO configuration RMW requires external serialization.
- **BCM system timer:** bounded CHI/CLO/CHI sampling prevents a torn low-word
  rollover. `Deadline` comparisons use signed wrapping distance and require
  samples within a half-range; relative delays are `1..=i32::MAX` microseconds.
  Only ARM compare channels 1/3 are exposed. `arm_after` clears its own CS bit,
  writes compare, and samples CLO again to detect programming races. Keep the
  controller IRQ masked until success. On `DeadlineElapsed`, keep it masked
  and handle/reprogram in software; the compare remains programmed. It is not
  the ARM architectural timer and does not claim to install alarms into boot.
- **BCM2836Interrupts:** legacy ARMCTRL peripheral banks 0..63 plus basic IRQs
  64..71. Enable/disable are write-one masks, with separate software ownership.
  `disable_all` establishes a known masked state. `dispatch` invokes each owned
  pending source at most once from one bounded snapshot and ignores summary/
  shortcut bits. A handler must acknowledge its peripheral. This is **not** a
  BCM2836 local per-core controller, a BCM2711/GIC implementation, vector setup,
  or proof that CPU interrupts are enabled.
- **DeviceManager<N>:** borrowed lifecycle and metadata with retained module
  leases, dependency preflight, rollback, and reverse shutdown; see [modules](modules.md).
- **Trace<T,N>:** records the first `N` completed reads/writes and a saturating
  overflow count while forwarding **all** underlying transactions unchanged.
  It adds timing overhead and neither simulates registers nor adds barriers.

## Property mailbox: DMA is explicitly unsafe

`PropertyMessage<WORDS>` aligns storage to 16 bytes and builds bounded word-based
messages (size, request code, concatenated tags, zero terminator). Tags reserve
response capacity, zero unused words, and keep reserved request-status bits zero.
`validate_response` checks the global status, framing/terminator, response bits,
and lengths before `response(expected_tag_id)` exposes words. Unsupported tags
and valid-but-truncated replies have distinct errors. Byte-granular tags, automatic
buffer growth, payload semantics, and authentication are outside this strict API.

`unsafe Mailbox::property_call(bus_address, polls)` uses **MAIL1_STATUS at 0x38**
to wait for outgoing space and **MAIL0_STATUS at 0x18** for incoming data. It
matches the entire address-plus-channel-eight token and bounds total status reads.
Only an exclusively owned receive mailbox is supported: unrelated tokens may be
consumed, so do not share it with outstanding transactions from other channels.

The caller must translate the CPU pointer to the firmware's bus address, pin the
message, clean caches/order stores before submitting, and invalidate/synchronize
before reading the response. A matching token does not validate the payload.
`SendTimeout` means nothing was sent; **ReplyTimeout means firmware may still be
writing the buffer**. Never move, free, reuse, or create references to such a
buffer until platform-specific recovery proves completion. The call is unsafe
precisely because a Rust borrow cannot end firmware DMA ownership on timeout.

## Verification and boot self-tests

`drivers::self_test()` and `modules::self_test()` return `Result<(), &'static str>`.
They use local fixed-capacity state, allocate nothing, are repeatable, and perform
**no peripheral access**. They can be called from a boot diagnostic without MMIO
initialization. They are not automatically wired into boot on this branch.

`sh tools/host-test.sh` exercises real driver logic against a strict, scripted
**fake** `RegisterIo`. Unexpected order/width/offset/value or unconsumed accesses
fail tests. RAM-backed volatile tests check the unsafe backend on aligned local
memory, not peripheral hardware. Host tests cover FIFO timeouts, UART errors,
baud rounding/init traces, both GPIO layouts and bank boundaries, timer rollover/
races, IRQ ownership/budgets, mailbox construction/validation/timeouts, manager
rollback, and trace overflow. A no_std AArch64 library compile checks the pinned
old-nightly API compatibility. None of this alone qualifies physical hardware.

## Register/protocol references

- [BCM2835 ARM peripherals](https://datasheets.raspberrypi.com/bcm2835/bcm2835-peripherals.pdf)
  (legacy UART/GPIO/system timer/interrupt layouts).
- [BCM2711 peripherals](https://datasheets.raspberrypi.com/bcm2711/bcm2711-peripherals.pdf)
  (GPIO pull-layout differences; not a claim of GIC support).
- [ARM PL011 technical reference](https://developer.arm.com/documentation/ddi0183/latest/)
  (FR/DR, baud, and initialization register semantics).
- [Firmware mailbox property interface](https://github.com/raspberrypi/firmware/wiki/Mailbox-property-interface)
  (alignment, reserved request bits, desired/truncated response lengths).
- [Linux v6.1 BCM mailbox transport](https://github.com/torvalds/linux/blob/v6.1/drivers/mailbox/bcm2835-mailbox.c)
  (separate read/write FIFO status registers).
- [Linux v6.1 BCM GPIO implementation](https://github.com/torvalds/linux/blob/v6.1/drivers/pinctrl/bcm/pinctrl-bcm2835.c)
  (legacy timing ambiguity and BCM2711 pull fields).
