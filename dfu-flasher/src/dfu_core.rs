use crate::dfu_status::{State, Status};
use crate::dfuse_command::DfuseCommand;
use crate::error::Error;
use std::convert::TryFrom;
use std::fs::File;
use std::io::Write;
use usbapi::UsbCore;
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

pub(crate) struct Dfu {
    usb: UsbCore,
    timeout: u32,
    interface: u16,
    xfer_size: u16,
}

impl Drop for Dfu {
    fn drop(&mut self) {
        self.usb
            .release_interface(self.interface as u32)
            .unwrap_or_else(|e| {
                eprintln!("Release interface failed with {}", e);
            });
    }
}
impl Dfu {
    pub fn from_bus_address(bus: u8, address: u8) -> Result<Self, Error> {
        let mut usb =
            UsbCore::from_bus_address(bus, address).map_err(|e| Error::USB("open".into(), e))?;
        usb.claim_interface(0).unwrap_or_else(|e| {
            eprintln!("Claim interface failed with {}", e);
        });
        usb.set_interface(0, 0).unwrap_or_else(|e| {
            eprintln!("Set interface failed with {}", e);
        });
        println!("{}", usb.get_descriptor_string_iface(6, 3));
        let timeout = 3000;
        Ok(Self {
            usb,
            timeout,
            interface: 0,
            xfer_size: 1024,
        })
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
                            eprintln!("Epipe try again");
                            std::thread::sleep(std::time::Duration::from_millis(3000));
                            continue;
                        }
                    }
                } else if let Error::InvalidControlResponse(_) = e {
                    eprintln!("Invalid control response");
                    std::thread::sleep(std::time::Duration::from_millis(3000));
                    continue;
                }
            }
            retries = 0;
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
        let mut s = self.get_status(10)?;
        while retries > 0 {
            if s.state != u8::from(&State::DfuDownloadBusy) {
                break;
            }
            println!("try again\n{}", s);
            std::thread::sleep(std::time::Duration::from_millis(100));
            retries -= 1;
            s = self.get_status(10)?;
        }

        if let Some(state) = wait_for_state {
            if s.state != u8::from(&state) {
                return Err(Error::InvalidState(s, state.clone()));
            }
        }

        println!("Ready:\n{}", s);
        if s.status != 0 {
            return Err(Error::InvalidStatus(s, 0));
        }
        Ok(s)
    }

    pub fn set_address(&mut self, address: u32) -> Result<Status, Error> {
        self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
        self.status_wait_for(0, Some(State::DfuDownloadIdle))
    }

    pub fn reset_stm32(&mut self, address: u32) -> Result<Status, Error> {
        self.set_address(address)?;
        self.dfuse_download(None, 2)?;
        self.get_status(100)
    }

    pub fn dfuse_get_commands(&mut self) -> Result<Vec<DfuseCommand>, Error> {
        self.abort_to_idle()?;
        let mut v = Vec::new();
        let cmds = &self.dfuse_upload(0)?;
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

    pub fn dfuse_mass_erase(&mut self) -> Result<(), Error> {
        self.status_wait_for(0, Some(State::DfuIdle))?;
        self.dfuse_download(Some(Vec::from(DfuseCommand::MassErase)), 0)?;
        Ok(())
    }

    pub fn dfu_upload(
        &mut self,
        file: &mut File,
        address: u32,
        mut length: u16,
    ) -> Result<(), Error> {
        self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
        self.status_wait_for(0, None)?;
        self.abort_to_idle()?;
        self.status_wait_for(0, None)?;
        let mut transfer = 2;
        while length != 0 {
            if length > 1024 {
                length -= 1024;
            } else {
                length = 0;
            }
            let v = self.dfuse_upload(transfer)?;
            file.write_all(&v)?;
            transfer += 1;
        }
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

    pub fn dfuse_do_dnload(&mut self, address: u32, file: &File) -> Result<(), Error> {
        panic!("not implemented");
    }

    pub fn dfuse_download(&mut self, buf: Option<Vec<u8>>, transaction: u16) -> Result<(), Error> {
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
                eprintln!("stalled on {:X?}", ctl);
                std::thread::sleep(std::time::Duration::from_millis(10));
                Ok(())
            }
            Err(e) => Err(Error::USBNix("Dfuse download".into(), e)),
            Ok(_) => Ok(()),
        }
    }

    fn dfuse_upload(&mut self, transaction: u16) -> Result<Vec<u8>, Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::new(
            ENDPOINT_IN | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_UPLOAD,
            transaction,
            self.interface,
            Some(vec![0 as u8; 1024]),
            self.timeout,
        );
        match self.usb.control_async_wait(ctl) {
            Err(e) => Err(Error::USBNix("Dfuse upload".into(), e)),
            Ok(buf) => Ok(buf),
        }
    }
}
