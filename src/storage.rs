use std::marker::PhantomData;
use std::borrow::Cow;

use embedded_svc::storage::RawStorage;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_sys::EspError;

pub trait EntryType {
    fn to_bytes(&self) -> Cow<[u8]>;
    fn from_bytes(bytes: Vec<u8>) -> Self;
}
impl EntryType for String {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Borrowed(self.as_bytes())
    }
    fn from_bytes(bytes: Vec<u8>) -> Self {
        String::from_utf8(bytes).unwrap()
    }
}

pub struct Entry<'a, T: EntryType> {
    nvs: &'a mut EspDefaultNvs,
    key: &'static str,
    _phantom: PhantomData<T>,
}
impl<T: EntryType> Entry<'_, T> {
    pub fn get(&self) -> Result<Option<T>, EspError> {
        let len = match self.nvs.len(self.key)? {
            Some(x) => x,
            None => return Ok(None),
        };

        let mut res = vec![0u8; len];
        assert_eq!(self.nvs.get_raw(self.key, &mut res)?.unwrap().len(), len);
        Ok(Some(T::from_bytes(res)))
    }
    pub fn set(&mut self, value: &T) -> Result<(), EspError> {
        self.nvs.set_raw(self.key, value.to_bytes().as_ref())?;
        Ok(())
    }
    pub fn clear(&mut self) -> Result<(), EspError> {
        self.nvs.remove(self.key)?;
        Ok(())
    }
}

macro_rules! impl_storage_entry {
    ($($name:ident ($key:ident) : $t:ty),*$(,)?) => {
        $(pub fn $name(&mut self) -> Entry<$t> { Entry { nvs: &mut self.nvs, key: stringify!($key), _phantom: PhantomData } })*

        pub fn clear_all(&mut self) -> Result<(), EspError> {
            $(self.$name().clear()?;)*
            Ok(())
        }
    }
}

pub struct StorageController {
    nvs: EspDefaultNvs,
}
impl StorageController {
    pub fn new(mut nvs: EspDefaultNvs) -> Result<Self, EspError> {
        const TEST_KEY: &'static str = "TEST_KEY"; // temp key to make sure nvs is configured properly and working
        const TEST_VAL: &'static [u8] = &[6, 127, 3, 200, 0, 128, 7, 255];
        let mut buf = [0u8; 16];

        nvs.remove(TEST_KEY)?;

        assert_eq!(nvs.contains(TEST_KEY)?, false);
        assert_eq!(nvs.len(TEST_KEY)?, None);
        assert_eq!(nvs.get_raw(TEST_KEY, &mut buf)?, None);

        nvs.set_raw(TEST_KEY, TEST_VAL)?;

        assert_eq!(nvs.contains(TEST_KEY)?, true);
        assert_eq!(nvs.len(TEST_KEY)?, Some(TEST_VAL.len()));
        assert_eq!(nvs.get_raw(TEST_KEY, &mut buf)?, Some(TEST_VAL));

        nvs.remove(TEST_KEY)?;

        assert_eq!(nvs.contains(TEST_KEY)?, false);
        assert_eq!(nvs.len(TEST_KEY)?, None);
        assert_eq!(nvs.get_raw(TEST_KEY, &mut buf)?, None);

        Ok(Self { nvs })
    }

    impl_storage_entry! {
        wifi_ap_ssid (wapssid): String,
        wifi_ap_pass (wappass): String,

        wifi_client_ssid (wclssid): String,
        wifi_client_pass (wclpass): String,

        peripherals (periph): String,

        project (proj): String,
    }
}
