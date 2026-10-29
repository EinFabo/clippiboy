//! The camera through Media Foundation.

use super::{CameraDevice, Error, OpenMode, Sink, Source};

pub fn list() -> Vec<CameraDevice> {
    Vec::new()
}

pub fn open(_device: &CameraDevice, _mode: OpenMode, _sink: Sink) -> Result<Box<dyn Source>, Error> {
    Err(Error::NoCamera)
}
