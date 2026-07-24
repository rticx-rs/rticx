#![no_std]
#![no_main]

// Ensure we halt the program on panic (if we don't mention this crate it won't
// be linked)
use panic_halt as _;


mod external_cipher;
mod external_hasher;
mod external_timer_led_driver;
mod external_uart_rx;

pub enum Command {
    Blink,
    Encrypt,
    Decrypt,
    Hash,
    Unknown,
}

#[rtic::app(device = rp_pico::hal::pac, peripherals = false, dispatchers = [SW0_IRQ])]
pub mod app {

    use core::sync::atomic::AtomicU32;
    use fugit::RateExtU32;
    use rp2040_hal::gpio::bank0::{Gpio0, Gpio1, Gpio25};
    use rp2040_hal::gpio::{FunctionSio, FunctionUart, Pin, PullDown, SioOutput};
    use rp2040_hal::timer::{Alarm, Alarm0};
    use rp2040_hal::uart::{
        DataBits, Reader as UartReader, StopBits, UartConfig, UartPeripheral, Writer,
    };
    use rp2040_hal::Clock;
    // Alias for our PAC crate
    use rp2040_hal::pac::{self};
    // Some traits we need

    use crate::Command;

    pub static TARGET_DURATION: AtomicU32 = AtomicU32::new(0);
    pub static TARGET_TICKS: AtomicU32 = AtomicU32::new(0);

    pub type UartRx = UartReader<
        pac::UART0,
        (
            Pin<Gpio0, FunctionUart, PullDown>,
            Pin<Gpio1, FunctionUart, PullDown>,
        ),
    >;

    pub type UartTx = Writer<
        pac::UART0,
        (
            Pin<Gpio0, FunctionUart, PullDown>,
            Pin<Gpio1, FunctionUart, PullDown>,
        ),
    >;

    pub type LedPin = Pin<Gpio25, FunctionSio<SioOutput>, PullDown>;

    /// External high-speed crystal on the Raspberry Pi Pico board is 12 MHz. Adjust
    /// if your board has a different frequency
    const XTAL_FREQ_HZ: u32 = 12_000_000u32;

    #[shared]
    struct Shared {
        uart_tx: UartTx, // shared between all tasks
        alarm: Alarm0,   // shared between CommandReceiverTask and CommandExecutorTask
    }

    /// Task the receives commands to over uart_rx
    /// see [crate::external_uart_rx] for task implementation
    #[task(
        binds = UART0_IRQ,
        priority = 1,
        shared = [uart_tx, alarm],
    )]
    pub struct CommandReceiverTask {
        pub data: heapless::String<30>,
        pub command: Command,
        pub read_command: bool, // tracks whether to read command or data ?
        pub uart_rx: UartRx,
    }

    /// Task that blinks the rp-pico onboard LED based on the duration and number of toggles provided by the master task
    /// see [crate::external_timer_led_driver] for task implementation
    #[task(
        binds = TIMER_IRQ_0,
        priority = 2,
        shared = [ uart_tx, alarm],
    )]
    pub struct CommandExecutorTask {
        pub led: LedPin,
    }

    /// Software task that encrypts the input text and  and prints the result in base64 format to uart_tx
    /// see [crate::external_cipher] for task implementation
    #[sw_task(
        priority = 3,
        shared = [uart_tx],
    )]
    pub struct Encryptor;

    /// Software task that decrypts the base64 input provided to it and prints the result to uart_tx
    /// see [crate::external_cipher] for task implementation
    #[sw_task(
        priority = 3,
        shared = [uart_tx],
    )]
    pub struct Decryptor;

    /// Software task that hashes the input provided to it and prints it to the uart_tx
    #[sw_task(
        priority = 3,
        shared = [uart_tx],
    )]
    pub struct Hasher;

    #[init]
    fn init() -> (Shared, TaskInits) {
        let mut device = pac::Peripherals::take().unwrap();

        // Initialization of the system clock.
        let mut watchdog = rp2040_hal::watchdog::Watchdog::new(device.WATCHDOG);

        // Configure the clocks - The default is to generate a 125 MHz system clock
        let clocks = rp2040_hal::clocks::init_clocks_and_plls(
            XTAL_FREQ_HZ,
            device.XOSC,
            device.CLOCKS,
            device.PLL_SYS,
            device.PLL_USB,
            &mut device.RESETS,
            &mut watchdog,
        )
        .ok()
        .unwrap();

        // The single-cycle I/O block controls our GPIO pins
        let sio = rp2040_hal::Sio::new(device.SIO);

        // Set the pins to their default state
        let pins = rp2040_hal::gpio::Pins::new(
            device.IO_BANK0,
            device.PADS_BANK0,
            sio.gpio_bank0,
            &mut device.RESETS,
        );

        // Set up UART on GP0 and GP1 (Pico pins 1 and 2) at 115200 baud rate
        let uart_pins = (pins.gpio0.into_function(), pins.gpio1.into_function());
        let uart = UartPeripheral::new(device.UART0, uart_pins, &mut device.RESETS)
            .enable(
                UartConfig::new(115200.Hz(), DataBits::Eight, None, StopBits::One),
                clocks.peripheral_clock.freq(),
            )
            .unwrap();

        let (mut uart_rx, mut uart_tx) = uart.split();
        uart_rx.enable_rx_interrupt(); // enable receiving interrupts
        uart_tx.disable_tx_interrupt(); // make sure tx interrupts are disabled

        // Configure GPIO25 as an output for driving the LED
        let led_pin = pins.gpio25.into_push_pull_output();

        let mut timer = rp2040_hal::Timer::new(device.TIMER, &mut device.RESETS, &clocks);
        let mut alarm0 = timer.alarm_0().unwrap();
        alarm0.enable_interrupt();

        uart_tx.write_full_blocking(b"Welcome to LedCommander Example\r\n");
        uart_tx.write_full_blocking(
            b"Enter the command and its arguments: <cmd> <arg1 arg2 ... arg_n>. Possible commands are:\r\n",
        );
        uart_tx.write_full_blocking(b"b <count> <duration> # toggles an led <count> times with <duration> between each toggle.\r\n");

        (
            Shared {
                uart_tx,
                alarm: alarm0,
            },
            TaskInits {
                command_receiver_task: CommandReceiverTask::init(uart_rx),
                command_executor_task: CommandExecutorTask::init(led_pin),
                hasher: Hasher::init(()),
                encryptor: Encryptor::init(()),
                decryptor: Decryptor::init(()),
            },
        )
    }
}
