#![no_main]
#![no_std]
#![allow(static_mut_refs)]
#![allow(non_snake_case)]

#[rtic::app(device = bsp)]
mod app {

    use core::arch::asm;

    use bsp::{
        clic::{Clic, Polarity, Trig},
        embedded_io::Write,
        fugit::ExtU32,
        mmap::{
            apb_timer::{TIMER0_ADDR, TIMER1_ADDR, TIMER2_ADDR, TIMER3_ADDR},
            CFG_BASE, PERIPH_CLK_DIV_OFS,
        },
        mtimer::{self, MTimer},
        read_u32, riscv, sprint, sprintln,
        tb::signal_pass,
        timer_group::{Periodic, Timer},
        uart::*,
        write_u32, Interrupt, CPU_FREQ,
    };
    use ufmt::derive::uDebug;

    #[shared]
    struct Shared {}

    #[cfg_attr(feature = "ufmt", derive(uDebug))]
    #[cfg_attr(not(feature = "ufmt"), derive(Debug))]
    struct TaskDef {
        // level is specified in RTIC task
        level: u8,
        period_ns: u32,
        duration_ns: u32,
    }

    const TEST_DURATION: mtimer::Duration = mtimer::Duration::micros(1_000);

    impl TaskDef {
        pub const fn new(level: u8, period_ns: u32, duration_ns: u32) -> Self {
            Self {
                period_ns,
                duration_ns,
                level,
            }
        }
    }

    const TEST_BASE_PERIOD_NS: u32 = 100_000;
    const TASK0: TaskDef = TaskDef::new(
        1,
        TEST_BASE_PERIOD_NS / 4,
        /* 25 ‰) */ TEST_BASE_PERIOD_NS / 40,
    );
    const TASK1: TaskDef = TaskDef::new(
        2,
        TEST_BASE_PERIOD_NS / 8,
        /* 12,5 ‰) */ TEST_BASE_PERIOD_NS / 80,
    );
    const TASK2: TaskDef = TaskDef::new(
        3,
        TEST_BASE_PERIOD_NS / 16,
        /* 5 ‰) */ TEST_BASE_PERIOD_NS / 200,
    );
    const TASK3: TaskDef = TaskDef::new(
        4,
        TEST_BASE_PERIOD_NS / 32,
        /* 2,5 ‰) */ TEST_BASE_PERIOD_NS / 400,
    );
    const PERIPH_CLK_DIV: u64 = 1;
    const CYCLES_PER_SEC: u64 = CPU_FREQ as u64 / PERIPH_CLK_DIV;
    const CYCLES_PER_MS: u64 = CYCLES_PER_SEC / 1_000;
    const CYCLES_PER_US: u32 = CYCLES_PER_MS as u32 / 1_000;
    // !!!: this would saturate to zero, so we must not use it. Use `X *
    // CYCLES_PER_US / 1_000 instead` and verify the output value is not saturated.
    /* const CYCLES_PER_NS: u64 = CYCLES_PER_US / 1_000; */

    static mut TASK0_COUNT: usize = 0;
    static mut TASK1_COUNT: usize = 0;
    static mut TASK2_COUNT: usize = 0;
    static mut TASK3_COUNT: usize = 0;

    const USE_PCS: bool = false;

    #[init]
    fn init() -> Shared {
        // Assert that periph clk div is as configured
        // !!!: this must be done prior to configuring any timing sensitive
        // peripherals
        write_u32(CFG_BASE + PERIPH_CLK_DIV_OFS, PERIPH_CLK_DIV as u32);

        let mut serial = ApbUart::init(CPU_FREQ, 115_200);
        sprintln!("[periodic_tasks (PCS={:?})]", USE_PCS);
        sprintln!(
            "Periph CLK div = {}",
            read_u32(CFG_BASE + PERIPH_CLK_DIV_OFS)
        );
        sprintln!(
            "Tasks: \r\n  {:?}\r\n  {:?}\r\n  {:?}\r\n  {:?}",
            TASK0,
            TASK1,
            TASK2,
            TASK3
        );
        sprintln!(
            "Test duration: {} us ({} ns)",
            TEST_DURATION.to_micros(),
            TEST_DURATION.to_nanos()
        );

        if USE_PCS {
            Clic::ie(Interrupt::Timer0Cmp).set_pcs(true);
            Clic::ie(Interrupt::Timer1Cmp).set_pcs(true);
            Clic::ie(Interrupt::Timer2Cmp).set_pcs(true);
            Clic::ie(Interrupt::Timer3Cmp).set_pcs(true);
        }

        // Make sure serial is done printing before proceeding to the test case
        unsafe { serial.flush().unwrap_unchecked() };

        // Use mtimer for timeout
        let mut mtimer = MTimer::instance().into_oneshot();

        let timers = &mut [
            Timer::init::<TIMER0_ADDR>().into_periodic(),
            Timer::init::<TIMER1_ADDR>().into_periodic(),
            Timer::init::<TIMER2_ADDR>().into_periodic(),
            Timer::init::<TIMER3_ADDR>().into_periodic(),
        ];

        timers[0].set_period(TASK0.period_ns.nanos());
        timers[1].set_period(TASK1.period_ns.nanos());
        timers[2].set_period(TASK2.period_ns.nanos());
        timers[3].set_period(TASK3.period_ns.nanos());

        // --- Test critical ---
        unsafe {
            asm!("fence");
            // clear mcycle, minstret at start of critical section
            asm!("csrw 0xB00, {0}", in(reg) 0x0);
            asm!("csrw 0xB02, {0}", in(reg) 0x0);
            /* !!! mcycle and minstret are missing write-methdods in BSP !!! */
        };

        // Test will end when MachineTimer fires
        mtimer.start(TEST_DURATION);

        // Start periodic timers
        timers.iter_mut().for_each(Periodic::start);

        Shared {}
    }

    #[task(binds = Timer0Cmp, priority=1)]
    struct T0 {}
    #[task(binds = Timer1Cmp, priority=2)]
    struct T1 {}
    #[task(binds = Timer2Cmp, priority=3)]
    struct T2 {}
    #[task(binds = Timer3Cmp, priority=4)]
    struct T3 {}

    impl RticTask for T0 {
        fn init() -> Self {
            Self {}
        }

        fn exec(&mut self) {
            unsafe {
                TASK0_COUNT += 1;
                core::arch::asm!(r#"
                    .rept {CNT}
                    nop
                    .endr
                "#, CNT = const TASK0.duration_ns * CYCLES_PER_US / 1_000);
            }
        }
    }

    impl RticTask for T1 {
        fn init() -> Self {
            Self {}
        }

        fn exec(&mut self) {
            unsafe {
                TASK1_COUNT += 1;
                core::arch::asm!(r#"
                    .rept {CNT}
                    nop
                    .endr
                "#, CNT = const TASK1.duration_ns * CYCLES_PER_US / 1_000);
            }
        }
    }

    impl RticTask for T2 {
        fn init() -> Self {
            Self {}
        }

        fn exec(&mut self) {
            unsafe {
                TASK2_COUNT += 1;
                core::arch::asm!(r#"
                    .rept {CNT}
                    nop
                    .endr
                "#, CNT = const TASK2.duration_ns * CYCLES_PER_US / 1_000);
            }
        }
    }

    impl RticTask for T3 {
        fn init() -> Self {
            Self {}
        }

        fn exec(&mut self) {
            unsafe {
                TASK3_COUNT += 1;
                core::arch::asm!(r#"
                    .rept {CNT}
                    nop
                    .endr
                "#, CNT = const TASK3.duration_ns * CYCLES_PER_US / 1_000);
            }
        }
    }

    #[task(binds = MachineTimer, priority=0xff)]
    struct Timeout {}

    impl RticTask for Timeout {
        fn init() -> Self {
            Self {}
        }

        fn exec(&mut self) {
            riscv::interrupt::disable();
            unsafe { asm!("fence") };
            // --- Test critical end ---

            let mut timer = MTimer::instance();
            timer.disable();

            unsafe {
                // Draw mtimer to max value to make sure all currently pending or in flight
                // TimerXCmp interrupts fall through.
                timer.set_counter(u64::MAX);

                // Disable all timers & interrupts, so no more instances will fire
                Timer::instance::<TIMER0_ADDR>().disable();
                Timer::instance::<TIMER1_ADDR>().disable();
                Timer::instance::<TIMER2_ADDR>().disable();
                Timer::instance::<TIMER3_ADDR>().disable();
                Clic::ip(Interrupt::MachineTimer).unpend();
                Clic::ip(Interrupt::Timer0Cmp).unpend();
                Clic::ip(Interrupt::Timer1Cmp).unpend();
                Clic::ip(Interrupt::Timer2Cmp).unpend();
                Clic::ip(Interrupt::Timer3Cmp).unpend();
            }

            // Clean up (RTIC won't do this for us unfortunately)
            tear_irq(Interrupt::Timer0Cmp);
            tear_irq(Interrupt::Timer1Cmp);
            tear_irq(Interrupt::Timer2Cmp);
            tear_irq(Interrupt::Timer3Cmp);
            tear_irq(Interrupt::MachineTimer);
            if USE_PCS {
                Clic::ie(Interrupt::Timer0Cmp).set_pcs(false);
                Clic::ie(Interrupt::Timer1Cmp).set_pcs(false);
                Clic::ie(Interrupt::Timer2Cmp).set_pcs(false);
                Clic::ie(Interrupt::Timer3Cmp).set_pcs(false);
            }

            let mut serial = unsafe { ApbUart::instance() };

            unsafe {
                let mcycle = riscv::register::mcycle::read64();
                let minstret = riscv::register::minstret::read64();

                sprintln!("cycles: {}", mcycle);
                sprintln!("instrs: {}", minstret);
                sprintln!(
                    "Task counts:\r\n{} | {} | {} | {}",
                    TASK0_COUNT,
                    TASK1_COUNT,
                    TASK2_COUNT,
                    TASK3_COUNT
                );
                let total_ns_in_task0 = TASK0.duration_ns * TASK0_COUNT as u32;
                let total_ns_in_task1 = TASK1.duration_ns * TASK1_COUNT as u32;
                let total_ns_in_task2 = TASK2.duration_ns * TASK2_COUNT as u32;
                let total_ns_in_task3 = TASK3.duration_ns * TASK3_COUNT as u32;
                sprintln!(
                    "Theoretical total duration spent in task workload (ns):\r\n{} | {} | {} | {} = {}",
                    total_ns_in_task0,
                    total_ns_in_task1,
                    total_ns_in_task2,
                    total_ns_in_task3,
                    total_ns_in_task0 + total_ns_in_task1 + total_ns_in_task2 + total_ns_in_task3,
                );

                // Make sure serial is done printing before proceeding to the next iteration
                serial.flush().unwrap_unchecked();
            }

            signal_pass(Some(&mut serial));
        }
    }

    /// Tear down the IRQ configuration to avoid side-effects for further testing
    pub fn tear_irq(irq: Interrupt) {
        Clic::ie(irq).disable();
        Clic::ctl(irq).set_level(0x0);
        Clic::attr(irq).set_shv(false);
        Clic::attr(irq).set_trig(Trig::Level);
        Clic::attr(irq).set_polarity(Polarity::Pos);
    }
}
