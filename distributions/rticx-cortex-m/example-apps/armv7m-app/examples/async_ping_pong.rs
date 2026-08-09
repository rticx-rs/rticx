#![no_std]
#![no_main]

use panic_halt as _;
use rticx_cortex_m as _;

#[rticx_cortex_m::app(
    device = stm32f0::stm32f0x0,
    dispatchers = [TIM6]
)]
mod app {
    use cortex_m_semihosting::{debug, hprintln};
    use rticx_async::channel::{Receiver, Sender};
    use rticx_async::make_channel;

    #[shared]
    struct Shared;

    #[init]
    fn system_init() -> (Shared, TaskInits) {
        let (tx1, rx1) = make_channel!(u32, 4);
        let (tx2, rx2) = make_channel!(u32, 4);
        (
            Shared,
            TaskInits {
                ping: Ping {
                    rx: rx1,
                    tx: tx2,
                },
                pong: Pong {
                    rx: rx2,
                    tx: tx1,
                },
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
            let _ = Pong::spawn(());
            let _ = Ping::spawn(());
            loop {
                cortex_m::asm::wfi();
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
            let r = self.rx.recv().await.expect("ping recv must succeed");
            hprintln!("ping: got {} from pong", r);
            if r == 7 {
                hprintln!("SUCCESS: ping received expected value 7");
                debug::exit(debug::EXIT_SUCCESS);
            } else {
                hprintln!("FAILURE: expected 7, got {}", r);
                debug::exit(debug::EXIT_FAILURE);
            }
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
}
