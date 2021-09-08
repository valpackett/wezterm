use anyhow::{anyhow, Error};
use filedescriptor::{FileDescriptor, Pipe};
use smithay_client_toolkit as toolkit;
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};
use toolkit::reexports::client::protocol::wl_data_offer::{Event as DataOfferEvent, WlDataOffer};
use toolkit::reexports::client::protocol::wl_data_source::WlDataSource;
use wayland_client::Attached;

#[derive(Default)]
pub struct CopyAndPaste {
    data_offer: Option<WlDataOffer>,
    pub(crate) last_serial: u32,
}

impl std::fmt::Debug for CopyAndPaste {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        fmt.debug_struct("CopyAndPaste")
            .field("last_serial", &self.last_serial)
            .field("data_offer", &self.data_offer.is_some())
            .finish()
    }
}

pub const TEXT_MIME_TYPE: &str = "text/plain;charset=utf-8";

impl CopyAndPaste {
    pub fn create() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Default::default()))
    }

    pub fn update_last_serial(&mut self, serial: u32) {
        if serial != 0 {
            self.last_serial = serial;
        }
    }

    pub fn get_clipboard_data(&mut self) -> anyhow::Result<FileDescriptor> {
        let offer = self
            .data_offer
            .as_ref()
            .ok_or_else(|| anyhow!("no data offer"))?;
        let pipe = Pipe::new().map_err(Error::msg)?;
        offer.receive(TEXT_MIME_TYPE.to_string(), pipe.write.as_raw_fd());
        Ok(pipe.read)
    }

    pub fn handle_data_offer(&mut self, event: DataOfferEvent, offer: WlDataOffer) {
        match event {
            DataOfferEvent::Offer { mime_type } => {
                if mime_type == TEXT_MIME_TYPE {
                    offer.accept(self.last_serial, Some(mime_type));
                    self.data_offer.replace(offer);
                } else {
                    // Refuse other mime types
                    offer.accept(self.last_serial, None);
                }
            }
            DataOfferEvent::SourceActions { source_actions } => {
                log::error!("Offer source_actions {:?}", source_actions);
            }
            DataOfferEvent::Action { dnd_action } => {
                log::error!("Offer dnd_action {:?}", dnd_action);
            }
            _ => {}
        }
    }

    pub fn confirm_selection(&mut self, offer: WlDataOffer) {
        self.data_offer.replace(offer);
    }

    pub fn set_selection(&mut self, source: &Attached<WlDataSource>) {
        use crate::connection::ConnectionOps;
        crate::Connection::get()
            .unwrap()
            .wayland()
            .pointer
            .data_device
            .set_selection(Some(&source), self.last_serial);
    }
}
