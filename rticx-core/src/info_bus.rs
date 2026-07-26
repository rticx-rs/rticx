use std::{
    any::Any,
    collections::{HashMap, hash_map::Entry},
    rc::Rc,
    sync::{Arc, Mutex},
};

use crate::errors;

/// A shared bus where compilation passes and backends can publish and get information from.
#[derive(Clone)]
pub struct InfoBus {
    infos: Arc<Mutex<HashMap<String, Rc<dyn Any>>>>,
}

impl InfoBus {
    pub(crate) fn new() -> Self {
        Self {
            infos: Default::default(),
        }
    }

    /// Publish an entry to the InfoBus
    /// The convention is that entry names are name spaced by the Compilation Pass name and then followed by the Type name.
    /// Example `rticx_core::App` or `rticx_core::Analysis`
    pub fn publish<T: Any>(&self, entry: impl ToString, value: T) -> Result<(), errors::Error> {
        let mut infos = self.infos.lock().expect("must be able to lock info bus");
        match infos.entry(entry.to_string()) {
            Entry::Occupied(_) => Err(errors::Error::EntryOccupied(entry.to_string())),
            Entry::Vacant(e) => {
                e.insert_entry(Rc::new(value));
                Ok(())
            }
        }
    }

    pub fn get<T: 'static>(&self, entry: &str) -> Result<Rc<T>, errors::Error> {
        let infos = self.infos.lock().expect("must be able to lock info bus");
        let e = infos
            .get(entry)
            .cloned()
            .ok_or(errors::Error::EntryNotFound(entry.to_string()))?;

        Rc::downcast::<T>(e).map_err(|_| errors::Error::InvalidTargetType(entry.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_and_publish_get_success() {
        let bus = InfoBus::new();

        // Publish a simple value
        let res = bus.publish("pass::my_int".to_string(), 42i32);
        assert!(res.is_ok());

        // Retrieve and downcast the value
        let val: Result<Rc<i32>, _> = bus.get("pass::my_int");
        assert!(val.is_ok());
        assert_eq!(*val.unwrap(), 42);
    }

    #[test]
    fn test_publish_duplicate_entry_fails() {
        let bus = InfoBus::new();
        let key = "rticx_sw_pass::app".to_string();

        assert!(bus.publish(key.clone(), "first_value").is_ok());

        // Attempting to publish to an occupied key should error
        let err = bus.publish(key.clone(), "second_value");
        assert!(matches!(err.unwrap_err(), errors::Error::EntryOccupied(_)));
    }

    #[test]
    fn test_get_nonexistent_entry_fails() {
        let bus = InfoBus::new();

        let res: Result<Rc<i32>, _> = bus.get("missing::key");
        assert!(matches!(res.unwrap_err(), errors::Error::EntryNotFound(_)));
    }

    #[test]
    fn test_get_type_mismatch_fails() {
        let bus = InfoBus::new();
        let key = "pass::data";

        bus.publish(key.to_string(), 100u32).unwrap();

        // Expect u32, but request String -> should error with InvalidTargetType
        let res: Result<Rc<String>, _> = bus.get(key);
        assert!(matches!(
            res.unwrap_err(),
            errors::Error::InvalidTargetType(_)
        ));
    }

    #[test]
    fn test_bus_clone_shares_state() {
        let bus1 = InfoBus::new();
        let bus2 = bus1.clone(); // Arc internally shared

        // Publish on bus1
        bus1.publish("shared::key".to_string(), true).unwrap();

        // Read from bus2
        let val: Rc<bool> = bus2.get("shared::key").unwrap();
        assert!(*val);
    }

    #[test]
    fn test_publish_complex_struct() {
        #[derive(Debug, PartialEq)]
        struct AppConfig {
            name: String,
            cores: u8,
        }

        let bus = InfoBus::new();
        let config = AppConfig {
            name: "RTIC App".to_string(),
            cores: 4,
        };

        bus.publish("rticx_sw_pass::config".to_string(), config)
            .unwrap();

        let retrieved: Rc<AppConfig> = bus.get("rticx_sw_pass::config").unwrap();
        assert_eq!(
            *retrieved,
            AppConfig {
                name: "RTIC App".to_string(),
                cores: 4,
            }
        );
    }
}
