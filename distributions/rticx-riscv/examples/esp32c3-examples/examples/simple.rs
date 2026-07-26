#![no_main]
#![no_std]

//! Very simple esp32c3 application for debugging purposes

esp_bootloader_esp_idf::esp_app_desc!();
use esp_backtrace as _;
use esp_println::println;
/// Include peripheral crate(s) that defines the vector table
use esp32c3::{self as _, Interrupt};

#[unsafe(no_mangle)]
fn main() -> ! {
    println!("program started");
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    // enable the software interrupt
    rticx_riscv::export::enable(Interrupt::FROM_CPU_INTR0, 5, 16);
    rticx_riscv::export::pend(Interrupt::FROM_CPU_INTR0);
    rticx_riscv::export::enable(Interrupt::FROM_CPU_INTR1, 3, 17);

    loop {
        println!("spinning...");
        riscv_semihosting::debug::exit(riscv_semihosting::debug::EXIT_SUCCESS);
    }
}

#[allow(non_snake_case)]
// #[unsafe(no_mangle)] // diabled due to warning
#[unsafe(export_name = "interrupt16")]
fn FROM_CPU_INTR0() {
    rticx_riscv::export::unpend(Interrupt::FROM_CPU_INTR0);
    rticx_riscv::export::pend(Interrupt::FROM_CPU_INTR1); // pend low prio interrupt
    println!("Hight prio interrupt called");
}

#[allow(non_snake_case)]
// #[unsafe(no_mangle)] // diabled due to warning
#[unsafe(export_name = "interrupt17")]
fn FROM_CPU_INTR1() {
    rticx_riscv::export::unpend(Interrupt::FROM_CPU_INTR1);
    println!("Low prio interupt called");
}
