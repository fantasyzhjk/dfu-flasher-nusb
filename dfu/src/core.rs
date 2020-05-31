use crate::dfuse_command::DfuseCommand;
use crate::error::Error;
use crate::memory_layout::MemoryLayout;
use crate::status::{State, Status};
use std::convert::TryFrom;
use std::fs::File;
use std::io::{Read, Write};
use std::str::FromStr;
use usbapi::{UsbCore, UsbDevice};
#[allow(dead_code)]
const DFU_DETACH: u8 = 0;
const DFU_DNLOAD: u8 = 1;
#[allow(dead_code)]
const DFU_UPLOAD: u8 = 2;
pub(crate) const DFU_GET_STATUS: u8 = 3;
const DFU_CLRSTATUS: u8 = 4;
#[allow(dead_code)]
const DFU_GETSTATE: u8 = 5;
#[allow(dead_code)]
const DFU_ABORT: u8 = 6;

#[derive(Debug)]
struct Transaction {
    transaction: u16,
    address: u32,
    pending: u32,
    xfer: u16,
    xfer_max: u16,
}

impl Transaction {
    fn new(address: u32, pending: u32, xfer_max: u16) -> Self {
        let mut t = Transaction {
            transaction: 2,
            address,
            pending,
            xfer: xfer_max,
            xfer_max,
        };
        t.set_xfer();
        t
    }

    fn set_xfer(&mut self) {
        if self.pending >= self.xfer_max as u32 {
            self.xfer = self.xfer_max;
            self.pending -= self.xfer_max as u32;
        } else {
            self.xfer = (self.pending % self.xfer_max as u32) as u16;
            self.pending = 0;
        }
    }
}

impl Iterator for Transaction {
    type Item = ();
    fn next(&mut self) -> Option<()> {
        if self.pending == 0 {
            self.xfer = 0;
            return None;
        }
        self.set_xfer();
        self.address += self.xfer as u32;
        self.transaction += 1;
        Some(())
    }
}

pub struct Dfu {
    usb: UsbCore,
    timeout: u32,
    interface: u16,
    xfer_size: u16,
    detached: bool,
    mem_layout: MemoryLayout,
}

impl Drop for Dfu {
    fn drop(&mut self) {
        if self.detached {
            return;
        }
        if let Err(_) = self.status_wait_for(0, Some(State::DfuIdle)) {
            log::debug!("Dfu was not idle abort to idle");
            self.abort_to_idle().unwrap_or_else(|e| {
                log::warn!("Abort to idle failed {}", e);
            });
        }
        self.usb
            .release_interface(self.interface as u32)
            .unwrap_or_else(|e| {
                log::warn!("Release interface failed with {}", e);
            });
    }
}

impl From<(UsbCore, MemoryLayout, u32, u32)> for Dfu {
    fn from((mut usb, mem_layout, iface, alt): (UsbCore, MemoryLayout, u32, u32)) -> Self {
        usb.claim_interface(iface).unwrap_or_else(|e| {
            log::warn!("Claim interface failed with {}", e);
        });
        usb.set_interface(iface, alt).unwrap_or_else(|e| {
            log::warn!("Set interface failed with {}", e);
        });
        let timeout = 3000;
        Self {
            usb,
            timeout,
            interface: 0,
            xfer_size: 1024,
            detached: false,
            mem_layout,
        }
    }
}

impl Dfu {
    pub fn from_bus_device(bus: u8, address: u8, iface: u32, alt: u32) -> Result<Self, Error> {
        let mut usb =
            UsbCore::from_bus_device(bus, address).map_err(|e| Error::USB("open".into(), e))?;
        let mem = MemoryLayout::from_str(&usb.get_descriptor_string_iface(0, 6))?;
        Ok(Dfu::from((usb, mem, iface, alt)))
    }

    pub fn get_status(&mut self, mut retries: u8) -> Result<Status, Error> {
        let mut status = Err(Error::Argument("Get status retries failed".into()));
        retries += 1;
        while retries > 0 {
            retries -= 1;
            status = Status::get(&mut self.usb, self.interface);
            if let Err(e) = &status {
                if let Error::USBNix(_, e) = e {
                    if let nix::Error::Sys(e) = e {
                        if *e == nix::errno::Errno::EPIPE {
                            log::warn!("Epipe try again");
                            std::thread::sleep(std::time::Duration::from_millis(3000));
                            continue;
                        }
                    }
                } else if let Error::InvalidControlResponse(e) = e {
                    log::warn!("retries {} Get status error cause '{}'", retries, e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
            } else {
                retries = 0;
            }
        }
        status
    }

    pub fn clear_status(&mut self) -> Result<(), Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::new(
            ENDPOINT_OUT | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_CLRSTATUS,
            0,
            self.interface,
            None,
            self.timeout,
        );
        let _ = self
            .usb
            .control(ctl)
            .map_err(|e| Error::USBNix("Control transfer".into(), e))?;

        Ok(())
    }

    pub fn detach(&mut self) -> Result<(), Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::new(
            ENDPOINT_OUT | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_DETACH,
            0,
            self.interface,
            None,
            self.timeout,
        );
        let _ = self
            .usb
            .control(ctl)
            .map_err(|e| Error::USBNix("Detach".into(), e))?;

        Ok(())
    }

    pub fn status_wait_for(
        &mut self,
        mut retries: u8,
        wait_for_state: Option<State>,
    ) -> Result<Status, Error> {
        retries += 1;
        let wait_for_state = if let Some(wait_for_state) = wait_for_state {
            wait_for_state
        } else {
            State::DfuDownloadBusy
        };
        let mut s = self.get_status(10)?;
        while retries > 0 {
            if s.state == u8::from(&wait_for_state) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            retries -= 1;
            s = self.get_status(10)?;
        }

        // check if expected state and return fail if not
        if s.state != u8::from(&wait_for_state) {
            return Err(Error::InvalidState(s, wait_for_state.clone()));
        }

        if s.status != 0 {
            return Err(Error::InvalidStatus(s, 0));
        }
        Ok(s)
    }

    pub fn set_address(&mut self, address: u32) -> Result<(), Error> {
        self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
        self.status_wait_for(0, Some(State::DfuDownloadIdle))?;
        Ok(())
    }

    pub fn reset_stm32(&mut self, address: u32) -> Result<(), Error> {
        //self.abort_to_idle()?;
        self.set_address(address)?;
        log::debug!("set done");
        //        self.abort_to_idle()?;
        //       log::debug!("abort done");
        self.dfuse_download(None, 2)?;
        log::debug!("dfuse None, 2 done");
        self.get_status(0).unwrap_or_else(|e| {
            log::warn!("get_status failed");
            Status::default()
        });
        self.detached = true;
        Ok(())
    }

    pub fn dfuse_get_commands(&mut self) -> Result<Vec<DfuseCommand>, Error> {
        self.abort_to_idle()?;
        let mut v = Vec::new();
        let cmds = &self.dfuse_upload(0, 1024)?;
        if let Some(cmd) = cmds.iter().next() {
            if *cmd != 0 {
                return Err(Error::InvalidControlResponse(format!(
                    "Get command {:X} {:X?}",
                    cmd, cmds
                )));
            }
        }
        for cmd in &cmds[1..] {
            v.push(DfuseCommand::try_from(*cmd)?)
        }
        Ok(v)
    }

    /// Verify flash using file
    pub fn verify(
        &mut self,
        file: &mut File,
        address: u32,
        length: Option<u32>,
    ) -> Result<(), Error> {
        let length = Self::get_length_from_file(file, length)?;
        self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
        self.status_wait_for(0, None)?;
        self.abort_to_idle()?;
        self.status_wait_for(0, Some(State::DfuIdle))?;
        let mut t = Transaction::new(address, length, self.xfer_size);
        while t.xfer > 0 {
            let address = t.address;
            self.flash_read_chunk(&mut t, |v| {
                let mut r = vec![0; v.len()];
                file.read(&mut r)?;
                let mut i2 = v.iter();
                for (i, byte) in r.iter().enumerate() {
                    if let Some(byte2) = i2.next() {
                        if byte == byte2 {
                            continue;
                        }
                    }
                    return Err(Error::Verify(address + i as u32));
                }
                if v.len() != r.len() {
                    return Err(Error::Verify(address + v.len() as u32));
                }
                Ok(())
            })?;
        }
        self.abort_to_idle()?;
        Ok(())
    }

    /// Erase pages from start address + length
    pub fn erase_pages(&mut self, mut address: u32, length: u32) -> Result<(), Error> {
        self.status_wait_for(0, Some(State::DfuIdle))?;
        let mut pages = self.mem_layout.num_pages(address, length)?;
        let page = self.mem_layout.address(address)?;
        // realign to beginning of page
        address = page.address;
        while pages > 0 {
            self.dfuse_download(Some(Vec::from(DfuseCommand::ErasePage(address))), 0)?;
            self.status_wait_for(0, Some(State::DfuDownloadBusy))?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle))?;
            pages -= 1;
            address += page.size;
        }
        Ok(())
    }

    /// Do mass erase of flash
    pub fn mass_erase(&mut self) -> Result<(), Error> {
        self.status_wait_for(0, Some(State::DfuIdle))?;
        self.dfuse_download(Some(Vec::from(DfuseCommand::MassErase)), 0)?;
        self.status_wait_for(0, Some(State::DfuDownloadBusy))?;
        self.status_wait_for(10, Some(State::DfuDownloadIdle))?;
        Ok(())
    }

    fn flash_read_chunk<F>(&mut self, t: &mut Transaction, mut f: F) -> Result<(), Error>
    where
        F: FnMut(Vec<u8>) -> Result<(), Error>,
    {
        log::debug!("{:X?}", t);
        let v = self.dfuse_upload(t.transaction, t.xfer)?;
        f(v)?;
        let _ = t.next().is_some();
        Ok(())
    }

    pub fn read_flash(&mut self, address: u32, buf: &mut [u8]) -> Result<usize, Error> {
        self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
        self.status_wait_for(0, None)?;
        self.abort_to_idle()?;
        self.status_wait_for(0, Some(State::DfuIdle))?;
        let mut len = 0;
        let mut size = buf.len();
        let mut t = Transaction::new(address, size as u32, self.xfer_size);
        while t.xfer > 0 {
            self.flash_read_chunk(&mut t, |v| {
                for b in v {
                    buf[len] = b;
                    len += 1;
                }
                Ok(())
            })?;
        }
        self.abort_to_idle()?;
        Ok(len)
    }

    /// Upload writes &file to flash.
    pub fn upload(&mut self, file: &mut File, address: u32, length: u32) -> Result<(), Error> {
        self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
        self.status_wait_for(0, None)?;
        self.abort_to_idle()?;
        self.status_wait_for(0, Some(State::DfuIdle))?;
        let mut t = Transaction::new(address, length, self.xfer_size);
        while t.xfer > 0 {
            self.flash_read_chunk(&mut t, |v| Ok(file.write_all(&v)?))?;
        }
        self.abort_to_idle()?;
        Ok(())
    }

    pub fn abort_to_idle(&mut self) -> Result<(), Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::new(
            ENDPOINT_OUT | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_ABORT,
            0,
            self.interface,
            None,
            self.timeout,
        );
        self.usb
            .control_async_wait(ctl)
            .map_err(|e| Error::USBNix("Abort to idle".into(), e))?;
        let s = self.get_status(0)?;
        if s.state != u8::from(&State::DfuIdle) {
            return Err(Error::InvalidState(s, State::DfuIdle));
        }
        Ok(())
    }

    fn get_length_from_file(file: &File, length: Option<u32>) -> Result<u32, Error> {
        let file_length = file.metadata()?.len() as u32;
        Ok(match length {
            Some(length) => {
                if file_length < length {
                    return Err(Error::Argument(format!(
                        "error on '{:?}' is {} bytes, but length is set to {} bytes",
                        file, file_length, length
                    )));
                }
                length
            }
            None => {
                if file_length == 0 {
                    return Err(Error::Argument(format!("File '{:?}' is empty", file)));
                }
                file_length
            }
        })
    }

    /// Download file to device using raw mode.
    /// If length is None it will read to file end.
    pub fn download_raw(
        &mut self,
        file: &mut File,
        address: u32,
        length: Option<u32>,
    ) -> Result<(), Error> {
        let mut length = Self::get_length_from_file(file, length)?;
        self.erase_pages(address, length)?;
        self.abort_to_idle()?;
        self.status_wait_for(0, Some(State::DfuIdle))?;
        let mut transaction = 2;
        let mut xfer;
        while length != 0 {
            if length >= self.xfer_size as u32 {
                xfer = self.xfer_size;
                length -= self.xfer_size as u32;
            } else {
                xfer = length as u16;
                length = 0;
            }
            log::debug!(
                "{}: 0x{:4X} xfer: {} length: {}",
                transaction,
                address,
                xfer,
                length
            );
            let mut buf = vec![0; xfer as usize];
            file.read(&mut buf)?;
            self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle))?;
            self.dfuse_download(Some(buf), transaction)?;
            self.status_wait_for(100, Some(State::DfuDownloadBusy))?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle))?;
            transaction += 1;
        }
        self.abort_to_idle()?;
        Ok(())
    }

    fn dfuse_download(&mut self, buf: Option<Vec<u8>>, transaction: u16) -> Result<(), Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::new(
            ENDPOINT_OUT | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_DNLOAD,
            transaction,
            self.interface,
            buf,
            self.timeout,
        );
        match self.usb.control(ctl.clone()) {
            Err(nix::Error::Sys(e)) if e == nix::errno::Errno::EPIPE => {
                log::warn!("stalled on {:X?}", ctl);
                std::thread::sleep(std::time::Duration::from_millis(10));
                Ok(())
            }
            Err(e) => Err(Error::USBNix("Dfuse download".into(), e)),
            Ok(_) => Ok(()),
        }
    }

    pub fn descriptors(&mut self) -> &Option<UsbDevice> {
        self.usb.descriptors()
    }

    pub fn memory_layout(&self) -> &MemoryLayout {
        &self.mem_layout
    }

    fn dfuse_upload(&mut self, transaction: u16, xfer: u16) -> Result<Vec<u8>, Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::new(
            ENDPOINT_IN | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_UPLOAD,
            transaction,
            self.interface,
            Some(vec![0 as u8; xfer as usize]),
            self.timeout,
        );
        match self.usb.control_async_wait(ctl) {
            Err(e) => Err(Error::USBNix("Dfuse upload".into(), e)),
            Ok(buf) => Ok(buf),
        }
    }
}
