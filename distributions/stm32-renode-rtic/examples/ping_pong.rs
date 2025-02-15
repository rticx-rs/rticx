//! Super simple RTIC multicore PingPong example:
//! Core 0 task dispatched by DMA_IRQ_0 -> pings -> Core 1 task dispatched by DMA_IRQ_1
//! and vice-versa !

//! When developing on VS code, one should enable #![allow(unused)], then once code is complete, this can be disabled
//! and one should rely on the output of the next two clippy commands instead of seeing vscode squiggles
//! core0= `RUSTFLAGS='--cfg core="0"' cargo clippy --example ping_pong`
//! core1= `RUSTFLAGS='--cfg core="1"' cargo clippy --example ping_pong`
#![allow(unused)]

#![no_std]
#![no_main]

#[rtic::app(device= [stm32f1xx_hal::pac, stm32f1xx_hal::pac], peripherals=false, dispatchers = [[TIM3],[TIM4]], cores=2)]
pub mod my_app {

    use cortex_m::asm;
    use panic_halt as _;

    #[cfg(core = "0")]
    use stm32f1xx_hal::pac::{self, USART2};
    #[cfg(core = "1")]
    use stm32f1xx_hal::pac::{self, USART3};

    use stm32f1xx_hal::{
        prelude::*,
        serial::{Config, Serial, Tx},
    };

    use core::fmt::Write;

    #[shared(core = 0)]
    struct SharedResources1 {
        tx: Tx<USART2>,
    }

    #[shared(core = 1)]
    struct SharedResources2 {
        tx2: Tx<USART3>,
    }

    const PING_PONG_DELAY: u32 = 30000000;

    // ======================================= CORE 0 ==============================================
    #[init(core = 0)]
    fn init_core0() -> SharedResources1 {
        // Get access to the device specific peripherals from the peripheral access crate
        let pac = unsafe { pac::Peripherals::steal() };

        // Take ownership over the raw flash and rcc devices and convert them into the corresponding
        // HAL structs
        let mut flash = pac.FLASH.constrain();
        let rcc = pac.RCC.constrain();

        // Freeze the configuration of all the clocks in the system and store the frozen frequencies in
        // `clocks`
        let clocks = rcc.cfgr.freeze(&mut flash.acr);

        // Prepare the alternate function I/O registers
        let mut afio = pac.AFIO.constrain();

        // Prepare the GPIOA peripheral
        let mut gpioa = pac.GPIOA.split();

        // USART2
        let tx = gpioa.pa2.into_alternate_push_pull(&mut gpioa.crl);
        let rx = gpioa.pa3;

        // Set up the usart device. Take ownership over the USART register and tx/rx pins. The rest of
        // the registers are used to enable and configure the device.
        let serial = Serial::new(
            pac.USART2,
            (tx, rx),
            &mut afio.mapr,
            Config::default().baudrate(9600.bps()),
            &clocks,
        );

        // Split the serial struct into a receiving and a transmitting part
        let (mut tx, _rx) = serial.split();

        writeln!(&mut tx, "core 0 started ....").unwrap();

        SharedResources1 { tx }
    }

    #[idle(core = 0)]
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
                asm::delay(120000000);
            }
        }
    }

    /// a Core0 task to be spawned by a task on Core1
    #[sw_task(priority = 1, spawn_by = 1, core = 0, shared = [tx])]
    struct Core0Task;
    impl RticSwTask for Core0Task {
        type SpawnInput = u32;
        fn init() -> Self {
            Self
        }

        fn exec(&mut self, ping: Self::SpawnInput) {
            assert_eq!(get_core_id(), 0); // assert that this is executing on core 0
            asm::delay(PING_PONG_DELAY); // add some delay for visualization
            let pong = ping + 1;
            self.shared().tx.lock(|tx| {
                writeln!(tx, "CORE0: Got ping {}, sending pong {}", ping, pong).unwrap();
            });
            if let Err(_e) = Core1Task::spawn_from(Self::current_core(), pong) {
                self.shared().tx.lock(|tx| {
                    writeln!(tx, "couldn't spawn task on core 1 from core 0").unwrap();
                });
            }

            // UNCOMMENT NEXT STATEMENT TO SEE THAT IT IS NOT ALLOWED
            // BECAUSE TASK IS MARKED BY `spawn_by = 1`. I.e only core 1 can spawn this task
            // let _ = Core0Task::spawn_from(Self::current_core(), 1);
        }
    }

    // ======================================= CORE 1 ==============================================

    #[init(core = 1)]
    fn system_init2() -> SharedResources2 {
        // Get access to the device specific peripherals from the peripheral access crate
        let pac = unsafe { pac::Peripherals::steal() };

        // Take ownership over the raw flash and rcc devices and convert them into the corresponding
        // HAL structs
        let mut flash = pac.FLASH.constrain();
        let rcc = pac.RCC.constrain();

        // Freeze the configuration of all the clocks in the system and store the frozen frequencies in
        // `clocks`
        let clocks = rcc.cfgr.freeze(&mut flash.acr);

        // Prepare the alternate function I/O registers
        let mut afio = pac.AFIO.constrain();

        // Prepare the GPIOB peripheral
        let mut gpiob = pac.GPIOB.split();

        // USART3
        let tx = gpiob.pb10.into_alternate_push_pull(&mut gpiob.crh);
        let rx = gpiob.pb11;

        // Set up the usart device. Take ownership over the USART register and tx/rx pins. The rest of
        // the registers are used to enable and configure the device.
        let serial = Serial::new(
            pac.USART3,
            (tx, rx),
            &mut afio.mapr,
            Config::default().baudrate(9600.bps()),
            &clocks,
        );

        // Split the serial struct into a receiving and a transmitting part
        let (mut tx2, _rx) = serial.split();

        writeln!(&mut tx2, "core 1 started ....").unwrap();

        SharedResources2 { tx2 }
    }

    /// a Core1 task to be spawned by a task on Core0
    #[sw_task(priority = 2, core = 1, spawn_by = 0, shared = [tx2])]
    struct Core1Task;
    impl RticSwTask for Core1Task {
        type SpawnInput = u32;
        fn init() -> Self {
            asm::delay(120000000); // some delay to make sure core 0 has started and is waiting for a message...
                                   // spawn task on core0 to begin ping pong
                                   // this is the correct place to initiate the ping-pong process since at this point we
                                   // know core 1 is awake and can start receiving interrupts
            Core0Task::spawn_from(Self::current_core(), 1).expect("Couldn't start task on core 0"); // this will be called during initalization
            Self
        }
        fn exec(&mut self, pong: Self::SpawnInput) {
            assert_eq!(get_core_id(), 1); // assert that this is executing on core 0
            asm::delay(PING_PONG_DELAY); // add some delay for visualization
            let ping = pong + 1;
            self.shared().tx2.lock(|tx| {
                writeln!(tx, "CORE1: Got pong {}, sending ping {}", pong, ping).unwrap();
            });
            if let Err(_e) = Core0Task::spawn_from(Self::current_core(), ping) {
                self.shared().tx2.lock(|tx| {
                    writeln!(tx, "couldn't spawn task on core 0 from core 1").unwrap();
                });
            }
        }
    }

    // no idle task is needed for core 1 in this example

    // ======================================== UTILS ==============================================

    /// Reads core number (0 or 1)
    const fn get_core_id() -> u32 {
        #[cfg(core = "0")]
        {
            0
        }
        #[cfg(not(core = "0"))]
        {
            1
        }
    }
}
