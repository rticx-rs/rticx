//! Empty PAC stand-in for the end-to-end fixture.
//!
//! The fixture's mock distribution generates no target-specific peripheral
//! code, so `#[app(device = mock_pac)]` only needs the crate to exist for the
//! generated `use mock_pac as _;`.
