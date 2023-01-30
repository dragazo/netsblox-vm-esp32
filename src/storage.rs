use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_sys::EspError;
use embedded_svc::storage::RawStorage;

pub struct Entry<'a> {
    nvs: &'a mut EspDefaultNvs,
    key: &'static str,
}
impl Entry<'_> {
    pub fn get(&self) -> Result<Option<Vec<u8>>, EspError> {
        let len = match self.nvs.len(self.key)? {
            Some(x) => x,
            None => return Ok(None),
        };

        let mut res = vec![0u8; len];
        assert_eq!(self.nvs.get_raw(self.key, &mut res)?.unwrap().len(), len);
        Ok(Some(res))
    }
    pub fn set(&mut self, value: &[u8]) -> Result<(), EspError> {
        self.nvs.set_raw(self.key, value)?;
        Ok(())
    }
    pub fn clear(&mut self) -> Result<(), EspError> {
        self.nvs.remove(self.key)?;
        Ok(())
    }
}

macro_rules! impl_storage_entry {
    ($($name:ident),*$(,)?) => {
        $(pub fn $name(&mut self) -> Entry { Entry { nvs: &mut self.nvs, key: stringify!($name) } })*

        pub fn clear(&mut self) -> Result<(), EspError> {
            $(self.$name().clear()?;)*
            Ok(())
        }
    }
}

pub struct StorageController {
    nvs: EspDefaultNvs,
}
impl StorageController {
    pub fn new(nvs: EspDefaultNvs) -> Self { Self { nvs } }

    impl_storage_entry! {
        wifi_ssid, wifi_pass,
    }
}
