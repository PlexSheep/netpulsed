use std::fs;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::errors::StoreError;
use crate::records::Check;

#[cfg(feature = "compression")]
use zstd;

/// The filename of the database, in [DB_PATH]
pub const DB_NAME: &str = "netpulse.store";
/// Path to the database of netpulse (combine with [DB_NAME])
pub const DB_PATH: &str = "/var/lib/netpulse";
#[cfg(feature = "compression")]
pub const ZSTD_COMPRESSION_LEVEL: i32 = 4;
pub const ENV_PATH: &str = "NETPULSE_STORE_PATH";

#[derive(Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct Store {
    checks: Vec<Check>,
}

impl Store {
    pub fn path() -> PathBuf {
        if let Some(var) = std::env::var_os(ENV_PATH) {
            let mut p = PathBuf::from(var);
            p.push(DB_NAME);
            p
        } else {
            PathBuf::from(format!("{DB_PATH}/{DB_NAME}"))
        }
    }

    fn new() -> Self {
        Self { checks: Vec::new() }
    }

    fn create() -> Result<Self, StoreError> {
        fs::create_dir_all(
            Self::path()
                .parent()
                .expect("the store path has no parent directory"),
        )?;

        let file = match fs::File::options()
            .read(false)
            .write(true)
            .append(false)
            .create_new(true)
            .open(Self::path())
        {
            Ok(file) => file,
            Err(err) => return Err(err.into()),
        };

        let store = Store::new();

        #[cfg(feature = "compression")]
        let mut writer = zstd::Encoder::new(file, ZSTD_COMPRESSION_LEVEL)?;
        #[cfg(not(feature = "compression"))]
        let mut writer = file;

        writer.write_all(&bincode::serialize(&store)?)?;
        writer.flush()?;
        Ok(store)
    }

    pub fn load_or_create() -> Result<Self, StoreError> {
        match Self::load() {
            Ok(store) => Ok(store),
            Err(err) => {
                if matches!(err, StoreError::DoesNotExist) {
                    Self::create()
                } else {
                    eprintln!("Error while trying to load the store: {err:#}");
                    Err(err)
                }
            }
        }
    }

    pub fn load() -> Result<Self, StoreError> {
        let file = match fs::File::options()
            .read(true)
            .write(false)
            .open(Self::path())
        {
            Ok(file) => file,
            Err(err) => match err.kind() {
                ErrorKind::NotFound => return Err(StoreError::DoesNotExist),
                _ => return Err(err.into()),
            },
        };

        #[cfg(feature = "compression")]
        let reader = zstd::Decoder::new(file)?;
        #[cfg(not(feature = "compression"))]
        let mut reader = file;

        Ok(bincode::deserialize_from(reader)?)
    }

    pub fn save(&self) -> Result<(), StoreError> {
        let mut file = match fs::File::options()
            .read(false)
            .write(true)
            .append(false)
            .create_new(false)
            .truncate(true)
            .create(false)
            .open(Self::path())
        {
            Ok(file) => file,
            Err(err) => match err.kind() {
                ErrorKind::NotFound => return Err(StoreError::DoesNotExist),
                _ => return Err(err.into()),
            },
        };

        #[cfg(feature = "compression")]
        let mut writer = zstd::Encoder::new(file, ZSTD_COMPRESSION_LEVEL)?;
        #[cfg(not(feature = "compression"))]
        let mut writer = file;

        writer.write_all(&bincode::serialize(&self)?)?;
        writer.flush()?;
        Ok(())
    }

    pub fn add_check(&mut self, check: impl Into<Check>) {
        self.checks.push(check.into());
    }
}