#![no_std]
#![no_main]

//! When developing on VS code, one should enable #![allow(unused)], then once code is complete, this can be disabled
//! and one should rely on the output of the next two clippy commands instead of seeing vscode squiggles
//! core0= `RUSTFLAGS='--cfg core="0"' cargo clippy --example hello_rtic_multi`
//! core1= `RUSTFLAGS='--cfg core="1"' cargo clippy --example hello_rtic_multi`
// #![allow(unused)]

#[rtic::app(device=stm32f1xx_hal::pac, peripherals=false, cores=2)]
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

    #[init(core = 0)]
    fn system_init() -> SharedResources1 {
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
        let (tx, _rx) = serial.split();

        SharedResources1 { tx }
    }

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
        let (tx2, _rx) = serial.split();

        SharedResources2 { tx2 }
    }

    #[task(binds = EXTI0 , priority = 3, shared = [tx])]
    pub struct MyTask {/* local resources */}
    impl RticTask for MyTask {
        fn init() -> Self {
            Self {}
        }

        fn exec(&mut self) {
            self.shared()
                .tx
                .lock(|tx| writeln!(tx, "My task called !").unwrap());
        }
    }

    #[idle(shared = [tx])]
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
                self.shared()
                    .tx
                    .lock(|tx| writeln!(tx, "core1: looping in idle... {}", self.count).unwrap());

                asm::delay(120000000);
            }
        }
    }

    #[idle(shared = [tx2])]
    struct MyIdleTask2 {
        /* local resources */
        count: u32,
    }
    impl RticIdleTask for MyIdleTask2 {
        fn init() -> Self {
            Self { count: 0 }
        }

        fn exec(&mut self) -> ! {
            loop {
                self.count += 1;
                self.shared()
                    .tx2
                    .lock(|tx2| writeln!(tx2, "core2: looping in idle... {}", self.count).unwrap());

                asm::delay(120000000);
            }
        }
    }
}
