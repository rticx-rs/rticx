#![no_std]
#![no_main]

use esp_backtrace as _;
use riscv_semihosting::debug::EXIT_SUCCESS;
use rtic_monotonics::esp32c3::prelude::*;
esp32c3_systimer_monotonic!(Mono);

esp_bootloader_esp_idf::esp_app_desc!();

#[rticx_riscv::app(
    device = esp32c3,
    dispatchers = [FROM_CPU_INTR0, FROM_CPU_INTR1]
)]
mod app {
    use super::*;
    use esp_println::println;
    use rticx_async::channel::{Receiver, Sender};
    use rticx_async::make_channel;

    #[shared]
    struct Shared;

    #[init]
    fn system_init() -> (Shared, TaskInits) {
        let _peripherals = esp_hal::init(esp_hal::Config::default());
        let pac = esp32c3::Peripherals::take().unwrap();
        let timer = pac.SYSTIMER;
        Mono::start(timer);

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

    #[post_init]
    fn post_init() {
        let _ = Periodic::spawn(10);
        let _ = Background::spawn(());
    }

    #[async_task(priority = 0, init = generated)]
    struct Background;
    impl RticAsyncTask for Background {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            loop {
                println!("running at priority 0 each second");
                Mono::delay(1000.millis()).await;
            }
        }
    }

    #[async_task(priority = 2)]
    struct Ping {
        rx: Receiver<'static, u32, 4>,
        tx: Sender<'static, u32, 4>,
    }
    impl RticAsyncTask for Ping {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            println!("ping: sending 1 to pong");
            self.tx.send(1).await.expect("ping send must succeed");
            println!("ping: waiting reply from pong");
            let r = self.rx.recv().await.expect("ping recv must succeed");
            println!("ping: got {} from pong", r);
        }
    }

    #[async_task(priority = 2)]
    struct Pong {
        rx: Receiver<'static, u32, 4>,
        tx: Sender<'static, u32, 4>,
    }
    impl RticAsyncTask for Pong {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            println!("pong: waiting for ping to send something...");
            let r = self.rx.recv().await.expect("pong recv must succeed");
            println!("pong: got {} from ping, sending reply 7", r);
            self.tx.send(7).await.expect("pong send must succeed");
            println!("pong: done");
        }
    }

    #[async_task(priority = 3, init = generated)]
    struct Periodic;
    impl RticAsyncTask for Periodic {
        type SpawnInput = u32;
        async fn exec(&mut self, count: u32) {
            println!("periodic task started");
            for i in 1..=count {
                println!("");
                println!("[{}/{}]: Spawning lower prio tasks ping and pong", i, count);
                let _ = Pong::spawn(());
                let _ = Ping::spawn(());

                println!("Sleeping for 500ms");
                Mono::delay(500.millis()).await;
            }
            println!("exiting");
            riscv_semihosting::debug::exit(EXIT_SUCCESS);
        }
    }
}
