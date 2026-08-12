#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_halt as _;
use cortex_m_rt as _;

#[rticx_rp2040::app(device = rp2040_hal::pac, dispatchers = [SW0_IRQ])]
mod app {
    use core::sync::atomic::{AtomicU32, Ordering};
    use cortex_m::asm;
    use embedded_hal::digital::v2::ToggleableOutputPin;
    use fugit::{MicrosDurationU32, RateExtU32};
    use heapless::String;
    use rp2040_hal::Clock;
    use rp2040_hal::gpio::bank0::{Gpio0, Gpio1, Gpio25};
    use rp2040_hal::gpio::{FunctionSio, FunctionUart, Pin, PullDown, SioOutput};
    use rp2040_hal::pac;
    use rp2040_hal::timer::{Alarm, Alarm0};
    use rp2040_hal::uart::{
        DataBits, Reader as UartReader, StopBits, UartConfig, UartPeripheral, Writer,
    };
    use rticx_async::channel::{Receiver, Sender};
    use rticx_async::make_channel;

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

    const XTAL_FREQ_HZ: u32 = 12_000_000u32;
    const ENC_KEY: &[u8; 13] = b"fd@aG692-d70s";

    #[shared]
    struct Shared {
        uart_tx: UartTx,
        alarm: Alarm0,
        target_blinks: u32,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let mut device = pac::Peripherals::take().unwrap();

        let mut watchdog = rp2040_hal::watchdog::Watchdog::new(device.WATCHDOG);

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

        let sio = rp2040_hal::Sio::new(device.SIO);

        let pins = rp2040_hal::gpio::Pins::new(
            device.IO_BANK0,
            device.PADS_BANK0,
            sio.gpio_bank0,
            &mut device.RESETS,
        );

        let uart_pins = (pins.gpio0.into_function(), pins.gpio1.into_function());
        let uart = UartPeripheral::new(device.UART0, uart_pins, &mut device.RESETS)
            .enable(
                UartConfig::new(115200.Hz(), DataBits::Eight, None, StopBits::One),
                clocks.peripheral_clock.freq(),
            )
            .unwrap();

        let (mut uart_rx, mut uart_tx) = uart.split();
        uart_rx.enable_rx_interrupt();
        uart_tx.disable_tx_interrupt();

        let led_pin = pins.gpio25.into_push_pull_output();

        let mut timer = rp2040_hal::Timer::new(device.TIMER, &mut device.RESETS, &clocks);
        let mut alarm0 = timer.alarm_0().unwrap();
        alarm0.enable_interrupt();

        let (tx, rx) = make_channel!(String<30>, 4);

        uart_tx.write_full_blocking(b"Welcome to Async Multitasker\r\n");
        uart_tx.write_full_blocking(b"Commands: b <count> <duration> | x <data>\r\n");
        uart_tx.write_full_blocking(b"  b = blink LED   x = process via async task\r\n");

        (
            Shared {
                uart_tx,
                alarm: alarm0,
                target_blinks: 0,
            },
            TaskInits {
                command_receiver_task: CommandReceiverTask::init((uart_rx, tx)),
                timed_led_toggler: TimedLedToggler::init(led_pin),
                async_processor: AsyncProcessor::init(rx),
            },
        )
    }

    #[post_init]
    fn post_init() {
        let _ = AsyncProcessor::spawn(());
    }

    enum Command {
        Blink,
        Process,
        Unknown,
    }

    #[task(
        binds = UART0_IRQ,
        priority = 1,
        shared = [uart_tx, alarm],
    )]
    struct CommandReceiverTask {
        data: heapless::String<30>,
        command: Command,
        read_command: bool,
        uart_rx: UartRx,
        tx: Sender<'static, String<30>, 4>,
    }

    impl RticTask for CommandReceiverTask {
        type InitArgs = (UartRx, Sender<'static, String<30>, 4>);
        fn init(args: Self::InitArgs) -> Self {
            Self {
                data: String::new(),
                read_command: true,
                command: Command::Unknown,
                uart_rx: args.0,
                tx: args.1,
            }
        }

        fn exec(&mut self) {
            let mut data = [0_u8; 48];
            let bytes = self.uart_rx.read_raw(&mut data).unwrap();

            self.shared()
                .uart_tx
                .lock(|uart| uart.write_full_blocking(&data[..bytes]));

            for b in &data[..bytes] {
                if self.read_command {
                    let cmd = match b {
                        b'b' => Command::Blink,
                        b'x' => Command::Process,
                        _ => Command::Unknown,
                    };
                    self.command = cmd;
                    self.read_command = false;
                } else if (b == &b'\n') || (b == &b'\r') {
                    self.run_command();
                    self.read_command = true;
                    self.data.clear();
                    self.command = Command::Unknown;
                } else if *b != b' ' || !self.data.is_empty() {
                    let _ = self.data.push(*b as char);
                }
            }
        }
    }

    impl CommandReceiverTask {
        fn run_command(&mut self) {
            match self.command {
                Command::Blink => {
                    let (blinks, duration) = self.data.split_once(' ').unwrap_or(("0", "0"));
                    let blinks: u32 = blinks.parse().unwrap_or(0);
                    let duration: u32 = duration.parse().unwrap_or(0);
                    TARGET_TICKS.store(blinks, Ordering::SeqCst);
                    TARGET_DURATION.store(duration, Ordering::SeqCst);
                    self.shared()
                        .uart_tx
                        .lock(|uart| uart.write_full_blocking(b"Starting blinky ...\r\n"));
                    self.shared().alarm.lock(|alarm| {
                        let _ = alarm.schedule(MicrosDurationU32::millis(duration));
                    });
                }
                Command::Process => {
                    self.shared().uart_tx.lock(|uart| {
                        uart.write_full_blocking(b"Sending to async processor: ");
                        uart.write_full_blocking(self.data.as_bytes());
                        uart.write_full_blocking(b"\r\n");
                    });
                    let _ = self.tx.try_send(self.data.clone());
                }
                Command::Unknown => self
                    .shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"Unknown command!\r\n")),
            }
        }
    }

    #[task(
        binds = TIMER_IRQ_0,
        priority = 2,
        shared = [uart_tx, alarm, target_blinks],
    )]
    pub struct TimedLedToggler {
        led: LedPin,
    }

    impl RticTask for TimedLedToggler {
        type InitArgs = LedPin;
        fn init(led: LedPin) -> Self {
            Self { led }
        }

        fn exec(&mut self) {
            let duration = TARGET_DURATION.load(Ordering::SeqCst);
            let blinks_left = TARGET_TICKS.load(Ordering::SeqCst);
            let blinks_left = blinks_left.saturating_sub(1);
            TARGET_TICKS.store(blinks_left, Ordering::SeqCst);

            let _ = self.led.toggle();

            if blinks_left == 0 {
                self.shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"finished pattern!\r\n"));
            }

            self.shared().alarm.lock(|alarm0| {
                if blinks_left != 0 {
                    let _ = alarm0.schedule(MicrosDurationU32::millis(duration));
                }
                alarm0.clear_interrupt();
            });
        }
    }

    #[async_task(
        priority = 3,
        shared = [uart_tx],
    )]
    struct AsyncProcessor {
        rx: Receiver<'static, String<30>, 4>,
    }

    impl RticAsyncTask for AsyncProcessor {
        type InitArgs = Receiver<'static, String<30>, 4>;
        type SpawnInput = ();

        fn init(rx: Self::InitArgs) -> Self {
            Self { rx }
        }

        async fn exec(&mut self, _input: ()) {
            self.shared().uart_tx.lock(|uart| {
                uart.write_full_blocking(b"Async: processor started, waiting for data...\r\n");
            });

            loop {
                let data = match self.rx.recv().await {
                    Ok(d) => d,
                    Err(_) => {
                        self.shared().uart_tx.lock(|uart| {
                            uart.write_full_blocking(b"Async: channel closed, exiting.\r\n");
                        });
                        break;
                    }
                };

                self.shared().uart_tx.lock(|uart| {
                    uart.write_full_blocking(b"Async: received \"");
                    uart.write_full_blocking(data.as_bytes());
                    uart.write_full_blocking(b"\", encrypting...\r\n");
                });

                let mut data = data;
                xor_cipher(unsafe { data.as_bytes_mut() });

                self.shared().uart_tx.lock(|uart| {
                    uart.write_full_blocking(b"Async: encrypted -> \"");
                    uart.write_full_blocking(data.as_bytes());
                    uart.write_full_blocking(b"\"\r\n");
                });

                self.shared().uart_tx.lock(|uart| {
                    uart.write_full_blocking(b"Async: computing hash...\r\n");
                });

                let hash = xor_hash(&data);

                let mut buf = itoa::Buffer::new();
                let hash_str = buf.format(hash);
                self.shared().uart_tx.lock(|uart| {
                    uart.write_full_blocking(b"Async: hash = ");
                    uart.write_full_blocking(hash_str.as_bytes());
                    uart.write_full_blocking(b"\r\n");
                });
            }
        }
    }

    fn xor_cipher(data: &mut [u8]) {
        for (i, byte) in data.iter_mut().enumerate() {
            let key_byte = ENC_KEY[i % ENC_KEY.len()];
            *byte ^= key_byte;
            asm::delay(1000);
        }
    }

    fn xor_hash(data: &String<30>) -> u32 {
        let mut hash = 0u32;
        for (i, &byte) in data.as_bytes().iter().enumerate() {
            let shift = (i % 4) * 8;
            hash ^= (byte as u32) << shift;
            asm::delay(1000);
        }
        hash
    }
}
