#![no_std]
#![no_main]

// Ensure we halt the program on panic (if we don't mention this crate it won't
// be linked)
use panic_halt as _;

#[rtic::app(device = rp_pico::hal::pac, peripherals = false)]
mod app {

    use core::sync::atomic::{AtomicU32, Ordering};
    use fugit::{MicrosDurationU32, RateExtU32};
    use heapless::String;
    use rp2040_hal::Clock;
    use rp2040_hal::gpio::bank0::{Gpio0, Gpio1, Gpio25};
    use rp2040_hal::gpio::{FunctionSio, FunctionUart, Pin, PullDown, SioOutput};
    use rp2040_hal::timer::{Alarm, Alarm0};
    use rp2040_hal::uart::{
        DataBits, Reader as UartReader, StopBits, UartConfig, UartPeripheral, Writer,
    };
    // Alias for our PAC crate
    use rp2040_hal::pac::{self};
    // Some traits we need
    use embedded_hal::digital::v2::ToggleableOutputPin;

    static TARGET_DURATION: AtomicU32 = AtomicU32::new(0);
    static TARGET_TICKS: AtomicU32 = AtomicU32::new(0);

    type UartRx = UartReader<
        pac::UART0,
        (
            Pin<Gpio0, FunctionUart, PullDown>,
            Pin<Gpio1, FunctionUart, PullDown>,
        ),
    >;

    type UartTx = Writer<
        pac::UART0,
        (
            Pin<Gpio0, FunctionUart, PullDown>,
            Pin<Gpio1, FunctionUart, PullDown>,
        ),
    >;

    type LedPin = Pin<Gpio25, FunctionSio<SioOutput>, PullDown>;

    /// External high-speed crystal on the Raspberry Pi Pico board is 12 MHz. Adjust
    /// if your board has a different frequency
    const XTAL_FREQ_HZ: u32 = 12_000_000u32;

    #[shared]
    struct Shared {
        uart_tx: UartTx,
        alarm: Alarm0,
        target_blinks: u32,
    }

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
        unsafe { pac::NVIC::unmask(pac::Interrupt::UART0_IRQ) };

        // Configure GPIO25 as an output for driving the LED
        let led_pin = pins.gpio25.into_push_pull_output();

        let mut timer = rp2040_hal::Timer::new(device.TIMER, &mut device.RESETS, &clocks);
        let mut alarm0 = timer.alarm_0().unwrap();
        alarm0.enable_interrupt();
        unsafe { pac::NVIC::unmask(pac::Interrupt::TIMER_IRQ_0) };

        uart_tx.write_full_blocking(b"Welcome to LedCommander Example\r\n");
        uart_tx.write_full_blocking(
            b"Enter the command and its arguments: <cmd> <arg1 arg2 ... arg_n>. Possible commands are:\r\n",
        );
        uart_tx.write_full_blocking(b"b <count> <duration> # toggles an led <count> times with <duration> between each toggle.\r\n");

        (
            Shared {
                uart_tx,
                alarm: alarm0,
                target_blinks: 0,
            },
            TaskInits {
                command_receiver_task: CommandReceiverTask::init(uart_rx),
                command_executor_task: CommandExecutorTask::init(led_pin),
            },
        )
    }

    enum Command {
        Blink,
        Unknown,
    }

    /// Task the receives commands to blink the led
    #[task(
        binds = UART0_IRQ,
        priority = 1,
        shared = [uart_tx, alarm],
    )]
    struct CommandReceiverTask {
        data: heapless::String<30>,
        command: Command,
        read_command: bool, // tracks whether to read command or data ?
        uart_rx: UartRx,
    }

    impl RticTask for CommandReceiverTask {
        type InitArgs = UartRx;
        fn init(uart_rx: UartRx) -> Self {
            Self {
                data: String::new(),
                read_command: true,
                command: Command::Unknown,
                uart_rx,
            }
        }

        fn exec(&mut self) {
            let mut data = [0_u8; 48];
            let bytes = self.uart_rx.read_raw(&mut data).unwrap();

            // echo back the read data
            self.shared()
                .uart_tx
                .lock(|uart| uart.write_full_blocking(&data[..bytes]));

            for b in &data[..bytes] {
                if self.read_command {
                    // read command
                    let cmd = match b {
                        b'b' => Command::Blink,
                        _ => Command::Unknown,
                    };
                    self.command = cmd;
                    self.read_command = false;
                } else if (b == &b'\n') || (b == &b'\r') {
                    // command finished
                    self.run_command();
                    self.read_command = true;
                    self.data.clear();
                    self.command = Command::Unknown;
                } else {
                    if *b != b' ' || !self.data.is_empty() {
                        // read command argument data
                        let _ = self.data.push(*b as char);
                    }
                }
            }
        }
    }

    impl CommandReceiverTask {
        fn run_command(&mut self) {
            // command finished
            match self.command {
                Command::Blink => {
                    // convert the buffers to values
                    let (blinks, duration) = self.data.split_once(' ').unwrap_or(("0", "0"));

                    let blinks: u32 = blinks.parse().unwrap_or(0);
                    let duration: u32 = duration.parse().unwrap_or(0);
                    TARGET_TICKS.store(blinks, Ordering::SeqCst);
                    TARGET_DURATION.store(duration, Ordering::SeqCst);
                    self.shared()
                        .uart_tx
                        .lock(|uart| uart.write_full_blocking(b"Starting blinky ...\r\n"));
                    // start the first alarm
                    self.shared().alarm.lock(|alarm| {
                        let _ = alarm.schedule(MicrosDurationU32::millis(duration));
                    });
                }
                Command::Unknown => self
                    .shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"Unknown command !\r\n")),
            }
        }
    }

    /// Task that blinks the rp-pico onboard LED and that send a message "LED ON!" and "LED OFF!" do USB-Serial.
    #[task(
        binds = TIMER_IRQ_0,
        priority = 2,
        shared = [ uart_tx, alarm, target_blinks],
    )]
    pub struct CommandExecutorTask {
        led: LedPin,
    }

    impl RticTask for CommandExecutorTask {
        type InitArgs = LedPin;
        fn init(led: LedPin) -> Self {
            Self { led }
        }

        fn exec(&mut self) {
            let duration = TARGET_DURATION.load(Ordering::SeqCst);
            let blinks_left = TARGET_TICKS.load(Ordering::SeqCst);
            let blinks_left = blinks_left.saturating_sub(1);
            TARGET_TICKS.store(blinks_left, Ordering::SeqCst);

            // toggle the LED
            let _ = self.led.toggle();

            if blinks_left == 0 {
                self.shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"finished pattern !\r\n"));
            }

            // don't forget to clear the interrrupt
            self.shared().alarm.lock(|alarm0| {
                if blinks_left != 0 {
                    let _ = alarm0.schedule(MicrosDurationU32::millis(duration));
                }
                alarm0.clear_interrupt();
            });
        }
    }
}
