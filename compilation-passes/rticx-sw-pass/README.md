# rticx-sw-pass

RTICv1-like Lightweight Software tasks compilation pass for the [RTICX](https://github.com/rticx-rs/rticx) framework.

Adds dispatchers, message queues, `spawn`, and `cross_spawn` support.

## Syntax

```rust
#[app(device = my_pac, dispatchers = [IRQ0, IRQ1])]
mod app {
    #[sw_task(priority = 2, capacity = 4)]
    struct Blinker;

    impl RticSwTask for Blinker {
        type SpawnInput = bool;

        fn exec(&mut self, input: bool) {
            // ...
        }
    }
}
```

### `#[app]` arguments consumed by this pass

- `dispatchers = [...]`: single-core: flat list of dispatcher interrupts.
- `dispatchers = [[...], [...]]`: multicore: one list per core, in core order.

### `#[sw_task(...)]` arguments

| Argument   | Type  | Default    | Description |
|------------|-------|------------|-------------|
| `priority` | `u16` | `0`        | Dispatcher priority group for this task. |
| `core`     | `u32` | `0`        | Core the task runs on. |
| `spawn_by` | `u32` | `core`     | Core allowed to spawn the task; any other value makes it a cross-core task spawnable with `cross_spawn`. |
| `capacity` | `usize` | `1`       | Number of pending spawns the task's input queue can hold. Must be at least 1. |


### Dispatcher assignment

Each distinct task priority level on a core consumes one dispatcher, so the
number of dispatchers per core must be at least the number of distinct
priorities. Dispatchers are assigned deterministically: priority groups are
sorted ascending and the dispatchers are assigned in declaration order: the
first declared dispatcher handles the lowest priority group.

## License

MIT
