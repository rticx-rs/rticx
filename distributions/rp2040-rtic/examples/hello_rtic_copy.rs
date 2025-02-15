//! Late resource initialization example
//! 1- define the type InitArgs in task implementation
//! 2- change signature of init() to init(args: <Self as RticTask>::InitArgs)
//! 3- the system init will as to return a tuple where the second item is a TaskInits struct, use that to pass explicit task initialization

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
    use rp2040_hal::pac::{self};

    type LedOutPin = Pin<Gpio25, FunctionSio<SioOutput>, PullDown>;
    static DELAY: u32 = 1000;

    #[shared]
    struct SharedResources;

    #[init]
    fn system_init() -> (SharedResources, TaskInits) {
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

        (
            SharedResources,
            TaskInits {
                blinker: Blinker::init((led_pin, alarm0)),
            },
        )
    }

    #[task(binds = TIMER_IRQ_0 , priority = 3)]
    struct Blinker {
        /* local resources */
        is_high: bool,
        counter: u16,
        led: LedOutPin,
        alarm: Alarm0,
    }

    impl RticTask for Blinker {
        type InitArgs = (LedOutPin, Alarm0);
        fn init((led, alarm): (LedOutPin, Alarm0)) -> Self {
            Self {
                is_high: false,
                counter: 0,
                led,
                alarm,
            }
        }

        fn exec(&mut self) {
            if self.is_high {
                let _ = self.led.set_low();
                self.is_high = false;
            } else {
                let _ = self.led.set_high();
                self.is_high = true;
            }

            self.counter += 1;
            let message = self.counter;
            if let Err(_e) = MyTask2::spawn(message) {
                error!("couldn't spawn task 2 for the first time ")
            }
            if let Err(_e) = MyTask2::spawn(message) {
                error!("couldn't spawn task 2 again")
            }

            let _ = self.alarm.schedule(MicrosDurationU32::millis(DELAY));
            self.alarm.clear_interrupt();
        }
    }

    #[sw_task(priority = 2)]
    struct MyTask2;
    impl RticSwTask for MyTask2 {
        fn init() -> Self {
            Self
        }

        type SpawnInput = u16;
        fn exec(&mut self, input: u16) {
            info!("task2 spawned with input {}", input);
        }
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
                info!("looping in idle... {}", self.count);
                asm::delay(12000000);
            }
        }
    }
}
