#![no_std]
#![no_main]

#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_GENERIC_03H;

#[rtic::app(device=rp2040_hal::pac, peripherals=false, dispatchers=[DMA_IRQ_0])]
pub mod my_app {

    use cortex_m::asm;
    use defmt::*;
    use defmt_rtt as _;
    use panic_probe as _;

    use rp2040_hal::fugit::MicrosDurationU32;
    use rp2040_hal::gpio::bank0::Gpio25;
    use rp2040_hal::gpio::{FunctionSio, Pin, PullDown, SioOutput};
    use rp2040_hal::timer::{Alarm, Alarm0};
    // Ensure we halt the program on panic (if we don't mention this crate it won't
    // be linked)

    use embedded_hal::digital::v2::OutputPin;
    // use panic_halt as _;
    use rp2040_hal::pac::{self};

    type LedOutPin = Pin<Gpio25, FunctionSio<SioOutput>, PullDown>;
    static DELAY: u32 = 1000;

    #[shared]
    struct SharedResources {
        alarm: Alarm0,
        led: LedOutPin,
    }

    #[init]
    fn system_init() -> SharedResources {
        let mut device = pac::Peripherals::take().unwrap();

        // Initialization of the system clock.
        let mut watchdog = rp2040_hal::watchdog::Watchdog::new(device.WATCHDOG);

        // Configure the clocks - The default is to generate a 125 MHz system clock
        let clocks = rp2040_hal::clocks::init_clocks_and_plls(
            // External high-speed crystal on the Raspberry Pi Pico board is 12 MHz
            12_000_000u32,
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

        // Configure GPIO25 as an output
        let led_pin = pins.gpio25.into_push_pull_output();
        // Configure Timer
        let mut timer = rp2040_hal::Timer::new(device.TIMER, &mut device.RESETS, &clocks);
        let mut alarm0 = timer.alarm_0().unwrap();
        alarm0.schedule(MicrosDurationU32::millis(DELAY)).unwrap();
        alarm0.enable_interrupt();

        SharedResources {
            alarm: alarm0,
            led: led_pin,
        }
    }

    #[task(binds = TIMER_IRQ_0 , priority = 3, shared = [alarm, led])]
    struct MyTask {
        /* local resources */
        is_high: bool,
        counter: u16,
    }

    impl RticTask for MyTask {
        fn init() -> Self {
            Self {
                is_high: false,
                counter: 0,
            }
        }

        fn exec(&mut self) {
            self.shared().led.lock(|led_pin| {
                if self.is_high {
                    let _ = led_pin.set_low();
                    self.is_high = false;
                } else {
                    let _ = led_pin.set_high();
                    self.is_high = true;
                }
            });

            self.counter += 1;
            let message = self.counter;
            if let Err(_e) = MyTask2::spawn(message) {
                error!("couldn't spawn task 2 for the first time ")
            }
            if let Err(_e) = MyTask2::spawn(message) {
                error!("couldn't spawn task 2 again")
            }

            self.shared().alarm.lock(|alarm0| {
                let _ = alarm0.schedule(MicrosDurationU32::millis(DELAY));
                alarm0.clear_interrupt();
            });
        }
    }

    #[sw_task(priority = 2, shared = [led])]
    struct MyTask2;
    impl RticSwTask for MyTask2 {
        type SpawnInput = u16;
        fn init() -> Self {
            Self
        }

        fn exec(&mut self, input: u16) {
            info!("task2 spawned with input {}", input);

            if let Err(_e) = MyTask7::spawn(input + 10) {
                error!("couldn't spawn task 7")
            }
        }
    }

    #[sw_task(priority = 2, shared = [led])]
    struct MyTask7;
    impl RticSwTask for MyTask7 {
        type SpawnInput = u16;
        fn init() -> Self {
            Self
        }

        fn exec(&mut self, input: u16) {
            info!("task7 spawned with input {}", input);
        }
    }

    #[task(binds = TIMER_IRQ_2 , priority = 1, shared = [alarm])]
    struct MyTask3;
    impl RticTask for MyTask3 {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {}
    }

    #[idle]
    struct MyIdleTask {
        /* local resources */
        count: u32,
    }
    impl RticIdleTask for MyIdleTask {
        fn init() -> Self {
            Self { count: 0 }
        }

        fn exec(&mut self) -> ! {
            loop {
                self.count += 1;
                // info!("looping in idle... {}", self.count);
                asm::delay(12000000);
                // asm::delay(120000000);
            }
        }
    }
}
