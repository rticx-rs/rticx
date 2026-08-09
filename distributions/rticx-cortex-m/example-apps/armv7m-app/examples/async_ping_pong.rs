#![no_std]
#![no_main]

use panic_halt as _;
use rtic_monotonics::systick::prelude::*;
systick_monotonic!(Mono, 1000);

#[rticx_cortex_m::app(
    device = stm32f0::stm32f0x0,
    dispatchers = [TIM6, TIM3]
)]
mod app {
    use super::*;
    use cortex_m_semihosting::{debug, hprint, hprintln};
    use rticx_async::channel::{Receiver, Sender};
    use rticx_async::make_channel;

    #[shared]
    struct Shared;

    #[init]
    fn system_init() -> (Shared, TaskInits) {
        // Setup clocks
        let core = unsafe { cortex_m::Peripherals::steal() };
        Mono::start(core.SYST, 10_000_000); // 10MHz

        let (tx1, rx1) = make_channel!(u32, 4);
        let (tx2, rx2) = make_channel!(u32, 4);

        (
            Shared,
            TaskInits {
                ping: Ping { rx: rx1, tx: tx2 },
                pong: Pong { rx: rx2, tx: tx1 },
            },
        )
    }

    #[idle]
    struct Idle;
    impl RticIdleTask for Idle {
        type InitArgs = ();
        fn init(_: ()) -> Self {
            Self
        }
        fn exec(&mut self) -> ! {
            let _ = Periodic::spawn(10);
            loop {
                for _ in 1..=80 {
                    hprint!("#"); // each '#' indicates that an interrupt occurred and all executors have finished their jobs
                    cortex_m::asm::wfi();
                }
                hprintln!("#");
            }
        }
    }

    #[async_task(priority = 2)]
    struct Ping {
        rx: Receiver<'static, u32, 4>,
        tx: Sender<'static, u32, 4>,
    }
    impl RticAsyncTask for Ping {
        type InitArgs = Self;
        type SpawnInput = ();
        fn init(s: Self::InitArgs) -> Self {
            s
        }
        async fn exec(&mut self, _input: ()) {
            hprintln!("ping: sending 1 to pong");
            self.tx.send(1).await.expect("ping send must succeed");
            hprintln!("ping: waiting reply from pong");
            let r = self.rx.recv().await.expect("ping recv must succeed");
            hprintln!("ping: got {} from pong", r);
        }
    }

    #[async_task(priority = 2)]
    struct Pong {
        rx: Receiver<'static, u32, 4>,
        tx: Sender<'static, u32, 4>,
    }
    impl RticAsyncTask for Pong {
        type InitArgs = Self;
        type SpawnInput = ();
        fn init(s: Self::InitArgs) -> Self {
            s
        }
        async fn exec(&mut self, _input: ()) {
            hprintln!("pong: waiting for ping to send something...");
            let r = self.rx.recv().await.expect("pong recv must succeed");
            hprintln!("pong: got {} from ping, sending reply 7", r);
            self.tx.send(7).await.expect("pong send must succeed");
            hprintln!("pong: done");
        }
    }

    #[async_task(priority = 3)]
    struct Periodic;
    impl RticAsyncTask for Periodic {
        type InitArgs = ();
        type SpawnInput = u32;
        fn init(_: Self::InitArgs) -> Self {
            Self
        }
        async fn exec(&mut self, count: u32) {
            hprintln!("periodic task started");
            for i in 1..=count {
                hprintln!("");
                hprintln!("[{}/{}]: Spawning lower prio tasks ping and pong", i, count);
                let _ = Pong::spawn(());
                let _ = Ping::spawn(());

                hprintln!("Sleeping for 500ms");
                Mono::delay(500.millis()).await;
            }
            hprintln!("exiting");
            debug::exit(debug::EXIT_SUCCESS);
        }
    }
}
