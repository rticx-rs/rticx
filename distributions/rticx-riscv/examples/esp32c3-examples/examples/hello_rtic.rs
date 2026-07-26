#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![no_main]
#![no_std]

esp_bootloader_esp_idf::esp_app_desc!();

#[rticx_riscv::app(device = esp32c3, dispatchers = [FROM_CPU_INTR0, FROM_CPU_INTR1])]
pub mod app {
    use esp_backtrace as _;
    use esp_hal::gpio::{Event, Input, InputConfig, Pull};
    use esp_println::println;
    use riscv_semihosting::debug::EXIT_SUCCESS;

    #[shared]
    struct Shared;

    #[init]
    fn init() -> (Shared, TaskInits) {
        println!("program started");

        let peripherals = esp_hal::init(esp_hal::Config::default());
        let mut button = Input::new(
            peripherals.GPIO9,
            InputConfig::default().with_pull(Pull::Up),
        );
        button.listen(Event::FallingEdge);
        println!("program handoff to RTICX");
        (
            Shared,
            TaskInits {
                gpio_handler: GpioHandler { button },
            },
        )
    }

    #[idle]
    struct IdleTask;
    impl RticIdleTask for IdleTask {
        fn init() -> Self {
            println!("idle init");
            Self
        }

        fn exec(&mut self) -> ! {
            println!("in idle task now about to spawn foo");
            Foo::spawn(()).unwrap();
            let target = 10;
            let mut i = 0;
            loop {
                if i >= target {
                    riscv_semihosting::debug::exit(EXIT_SUCCESS);
                } else {
                    i += 1;
                }
                println!("spinning... {i}");
            }
        }
    }

    #[task(binds = GPIO, priority=6)]
    struct GpioHandler {
        button: Input<'static>,
    }

    impl RticTask for GpioHandler {
        type InitArgs = Self;
        fn init(init: Self) -> Self {
            init
        }

        fn exec(&mut self) {
            self.button.clear_interrupt();
            println!("button");
        }
    }

    #[sw_task(priority = 5)]
    struct Foo;

    impl RticSwTask for Foo {
        type SpawnInput = ();

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, _input: ()) {
            println!("Foo started, calling to Bar");
            Bar::spawn(()).unwrap(); //enqueue low prio task
            println!("Inside high prio task, press button now!");
            let mut x = 0;
            while x < 300000 {
                x += 1; //burn cycles
                esp_hal::riscv::asm::nop();
            }
            println!("Leaving high prio task.");
        }
    }

    #[sw_task(priority = 3)]
    struct Bar;

    impl RticSwTask for Bar {
        type SpawnInput = ();

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, _input: ()) {
            println!("Inside low prio task, press button now!");
            let mut x = 0;
            while x < 300000 {
                x += 1; //burn cycles
                esp_hal::riscv::asm::nop();
            }
            println!("Leaving low prio task.");
        }
    }
}
