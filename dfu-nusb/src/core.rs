use crate::dfuse_command::DfuseCommand;
use crate::error::Error;
use crate::memory_layout::MemoryLayout;
use crate::status::{State, Status};
use std::convert::TryFrom;
use std::fs::File;
use std::io::{Read, Write};
use std::str::FromStr;
use std::time::Duration;
use futures_lite::future::block_on;
use nusb;
use nusb::descriptors::language_id::US_ENGLISH;
use nusb::descriptors::Descriptor;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
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

pub struct DfuDescriptor {
    pub attributes: u8,
    pub detach_timeout: u16,
    pub transfer_size: u16,
    pub dfu_version: u8,
}

impl DfuDescriptor {
    fn new(desc: Descriptor) -> Option<Self> {
        let mut iter = desc.iter();
        // length
        if *iter.next()? != 9 {
            return None;
        }

        // type
        if *iter.next()? != 33 {
            return None;
        }

        Some(DfuDescriptor {
            attributes: *iter.next()?,
            detach_timeout: *iter.next()? as u16 | (*iter.next()? as u16) << 8,
            transfer_size: *iter.next()? as u16 | (*iter.next()? as u16) << 8,
            dfu_version: *iter.next()?,
        })
    }
}

pub struct Dfu {
    usb: nusb::Device,
    interface: nusb::Interface,
    detached: bool,
    dfu_descriptor: DfuDescriptor,
    mem_layout: MemoryLayout,
}

impl Drop for Dfu {
    fn drop(&mut self) {
        if self.detached {
            return;
        }
        if block_on(self.status_wait_for(0, Some(State::DfuIdle))).is_err() {
            log::debug!("Dfu was not idle abort to idle");
            block_on(self.abort_to_idle()).unwrap_or_else(|e| {
                log::warn!("Abort to idle failed {}", e);
            });
        }
        // self.usb
        //     .release_interface(self.interface as u32)
        //     .unwrap_or_else(|e| {
        //         log::warn!("Release interface failed with {}", e);
        //     });
    }
}

impl Dfu {
    fn setup(usb: nusb::Device, iface_index: u8, alt_index: u8) -> Result<Self, Error> {
        let interface = usb.claim_interface(iface_index).map_err(|e| {
            log::error!("Claim interface failed with {}", e);
            Error::USB("Claim interface failed".into(), e)
        })?;

        let conf = usb.active_configuration().map_err(|_| {
            Error::DeviceNotFound("Missing active configuration".to_string())
        })?;

        let alt = conf.interface_alt_settings().find(|s| {
            s.interface_number() == iface_index && s.alternate_setting() == alt_index
        }).ok_or_else(|| {
            Error::DeviceNotFound("Missing configuration alt setting".to_string())
        })?;

        let mem_layout = MemoryLayout::from_str(
            &alt.string_index().map(|i| usb.get_string_descriptor(i, US_ENGLISH, Duration::from_secs(1)).unwrap()).ok_or_else(|| {
                Error::DeviceNotFound("Missing configuration descriptor".to_string())
            })?
        )?;
        
        let dfu_descriptor = conf.descriptors()
        .find(|desc| desc.descriptor_type() == 33)
        .map(|desc| DfuDescriptor::new(desc.clone())).ok_or_else(|| {
            Error::DeviceNotFound("Missing configuration dfu transfer descriptor".to_string())
        })?.unwrap();

        interface.set_alt_setting(alt_index).unwrap();

        log::debug!("Transfer size: {} bytes", dfu_descriptor.transfer_size);
        Ok(Self {
            usb,
            interface,
            dfu_descriptor,
            detached: false,
            mem_layout,
        })
    }

    pub async fn from_bus_device(bus: u8, dev_addr: u8, iface_index: u8, alt: u8) -> Result<Self, Error> {
        
        let device = nusb::list_devices()
        .unwrap()
        .find(|dev| dev.bus_number() == bus && dev.device_address() == dev_addr)
        .expect("device not connected");

        let usb = device.open().map_err(|e| Error::USB("open".into(), e))?;

        let mut dfu = Dfu::setup(usb, iface_index, alt)?;
        dfu.abort_to_idle_clear_once().await?;
        Ok(dfu)
    }

    pub async fn from_vid_pid(vid: u16, pid: u16, iface_index: u8, alt: u8) -> Result<Self, Error> {
        
        let device = nusb::list_devices()
        .unwrap()
        .find(|dev| dev.vendor_id() == vid && dev.product_id() == pid)
        .expect("device not connected");

        let usb = device.open().map_err(|e| Error::USB("open".into(), e))?;

        let mut dfu = Dfu::setup(usb, iface_index, alt)?;
        dfu.abort_to_idle_clear_once().await?;
        Ok(dfu)
    }

    pub async fn get_status(&mut self, mut retries: u8) -> Result<Status, Error> {
        let mut status = Err(Error::Argument("Get status retries failed".into()));
        retries += 1;
        while retries > 0 {
            retries -= 1;
            status = Status::get(&self.interface).await;
            if let Err(e) = &status {
                if let Error::USB(_, e) = e {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        log::warn!("Epipe try again");
                        tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                        continue;
                    }
                } else if let Error::InvalidControlResponse(e) = e {
                    log::warn!("retries {} Get status error cause '{}'", retries, e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            } else {
                retries = 0;
            }
        }
        status
    }

    pub async fn clear_status(&mut self) -> Result<(), Error> {
        self.interface.control_out(ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: DFU_CLRSTATUS,
            value: 0,
            index: self.interface.interface_number() as u16,
            data: &[],
        }).await.into_result().map_err(|e| Error::USB("Control transfer".into(), e.into()))?;
        Ok(())
    }

    pub async fn detach(&mut self) -> Result<(), Error> {
        self.interface.control_out(ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: DFU_DETACH,
            value: 0,
            index: self.interface.interface_number() as u16,
            data: &[],
        }).await.into_result().map_err(|e| Error::USB("Detach".into(), e.into()))?;
        Ok(())
    }

    pub async fn status_wait_for(
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
        let mut s = self.get_status(10).await?;
        while retries > 0 {
            if s.state == u8::from(&wait_for_state) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            retries -= 1;
            s = self.get_status(10).await?;
        }

        // check if expected state and return fail if not
        if s.state != u8::from(&wait_for_state) {
            return Err(Error::InvalidState(s, wait_for_state));
        }

        if s.status != 0 {
            return Err(Error::InvalidStatus(s, 0));
        }
        Ok(s)
    }

    pub async fn set_address(&mut self, address: u32) -> Result<(), Error> {
        self.dfuse_download(Vec::from(DfuseCommand::SetAddress(address)), 0).await?;
        self.status_wait_for(0, Some(State::DfuDownloadIdle)).await?;
        Ok(())
    }

    pub async fn reset_stm32(&mut self, address: u32) -> Result<(), Error> {
        //self.abort_to_idle()?;
        self.set_address(address).await?;
        log::debug!("set done");
        //        self.abort_to_idle()?;
        //       log::debug!("abort done");
        self.dfuse_download(Vec::new(), 2).await?;
        log::debug!("dfuse None, 2 done");
        self.get_status(0).await.unwrap_or_else(|e| {
            log::warn!("get_status failed cause {}", e);
            Status::default()
        });
        self.detached = true;
        Ok(())
    }

    pub async fn dfuse_get_commands(&mut self) -> Result<Vec<DfuseCommand>, Error> {
        self.abort_to_idle().await?;
        let mut v = Vec::new();
        let cmds = &self.dfuse_upload(0, 1024).await?;
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
    pub async fn verify(
        &mut self,
        file: &mut File,
        address: u32,
        length: u32,
    ) -> Result<(), Error> {
        self.dfuse_download(Vec::from(DfuseCommand::SetAddress(address)), 0).await?;
        self.status_wait_for(0, None).await?;
        self.abort_to_idle().await?;
        self.status_wait_for(0, Some(State::DfuIdle)).await?;
        let mut t = Transaction::new(address, length, self.dfu_descriptor.transfer_size);
        while t.xfer > 0 {
            let address = t.address;
            self.flash_read_chunk(&mut t, |v| {
                let mut r = vec![0; v.len()];
                file.read_exact(&mut r)?;
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
            }).await?;
        }
        self.abort_to_idle().await?;
        Ok(())
    }

    /// Erase pages from start address + length
    pub async fn erase_pages(&mut self, mut address: u32, length: u32) -> Result<(), Error> {
        self.status_wait_for(0, Some(State::DfuIdle)).await?;
        let mut pages = self.mem_layout.num_pages(address, length)?;
        let page = self.mem_layout.address(address)?;
        // realign to beginning of page
        address = page.address;
        while pages > 0 {
            self.dfuse_download(Vec::from(DfuseCommand::ErasePage(address)), 0).await?;
            self.status_wait_for(0, Some(State::DfuDownloadBusy)).await?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle)).await?;
            pages -= 1;
            address += page.size;
        }
        Ok(())
    }

    /// Do mass erase of flash
    pub async fn mass_erase(&mut self) -> Result<(), Error> {
        self.status_wait_for(0, Some(State::DfuIdle)).await?;
        self.dfuse_download(Vec::from(DfuseCommand::MassErase), 0).await?;
        self.status_wait_for(0, Some(State::DfuDownloadBusy)).await?;
        self.status_wait_for(10, Some(State::DfuDownloadIdle)).await?;
        Ok(())
    }

    async fn flash_read_chunk<F>(&mut self, t: &mut Transaction, mut f: F) -> Result<(), Error>
    where
        F: FnMut(Vec<u8>) -> Result<(), Error>,
    {
        log::debug!("{:X?}", t);
        let v = self.dfuse_upload(t.transaction, t.xfer).await?;
        f(v)?;
        let _ = t.next().is_some();
        Ok(())
    }

    pub async fn write_flash_from_slice(&mut self, address: u32, buf: &[u8]) -> Result<usize, Error> {
        let mut length = buf.len() as u32;
        self.erase_pages(address, length).await?;
        self.abort_to_idle().await?;
        self.status_wait_for(0, Some(State::DfuIdle)).await?;
        let mut transaction = 2;
        let mut xfer;
        if length >= self.dfu_descriptor.transfer_size as u32 {
            panic!(
                "FIXME write_flash_from_slice only allow xfer size max {}",
                self.dfu_descriptor.transfer_size
            );
        }
        while length != 0 {
            if length >= self.dfu_descriptor.transfer_size as u32 {
                xfer = self.dfu_descriptor.transfer_size;
                length -= self.dfu_descriptor.transfer_size as u32;
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
            self.dfuse_download(Vec::from(DfuseCommand::SetAddress(address)), 0).await?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle)).await?;
            self.dfuse_download(buf.into(), transaction).await?;
            self.status_wait_for(100, Some(State::DfuDownloadBusy)).await?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle)).await?;
            transaction += 1;
        }
        self.abort_to_idle().await?;
        Ok(length as usize)
    }

    pub async fn read_flash_to_slice(&mut self, address: u32, buf: &mut [u8]) -> Result<usize, Error> {
        self.dfuse_download(Vec::from(DfuseCommand::SetAddress(address)), 0).await?;
        self.status_wait_for(0, None).await?;
        self.abort_to_idle().await?;
        self.status_wait_for(0, Some(State::DfuIdle)).await?;
        let mut len = 0;
        let size = buf.len();
        let mut t = Transaction::new(address, size as u32, self.dfu_descriptor.transfer_size);
        while t.xfer > 0 {
            self.flash_read_chunk(&mut t, |v| {
                for b in v {
                    buf[len] = b;
                    len += 1;
                }
                Ok(())
            }).await?;
        }
        self.abort_to_idle().await?;
        Ok(len)
    }

    /// Upload read flash and store it in file.
    pub async fn upload(&mut self, file: &mut File, address: u32, length: u32) -> Result<(), Error> {
        self.dfuse_download(Vec::from(DfuseCommand::SetAddress(address)), 0).await?;
        self.status_wait_for(0, None).await?;
        self.abort_to_idle().await?;
        self.status_wait_for(0, Some(State::DfuIdle)).await?;
        let mut t = Transaction::new(address, length, self.dfu_descriptor.transfer_size);
        while t.xfer > 0 {
            self.flash_read_chunk(&mut t, |v| Ok(file.write_all(&v)?)).await?;
        }
        self.abort_to_idle().await?;
        Ok(())
    }

    pub async fn abort_to_idle_clear_once(&mut self) -> Result<(), Error> {
        let s = self.get_status(0).await?;
        if s.state == u8::from(&State::DfuIdle) {
            log::debug!("Status is {}", s.state);
            return Ok(());
        }

        self.interface.control_out(ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: DFU_ABORT,
            value: 0,
            index: self.interface.interface_number() as u16,
            data: &[],
        }).await.into_result().map_err(|e| Error::USB("Abort to idle".into(), e.into()))?;
    
        let s = self.get_status(0).await?;
        // try clear and read again in case of wrong state
        log::debug!("Status is after one abort {}", s.state);
        if s.state != u8::from(&State::DfuIdle) {
            self.clear_status().await?;
            log::debug!("Status cleared");
            self.get_status(0).await?;
        }
        Ok(())
    }

    pub async fn abort_to_idle(&mut self) -> Result<(), Error> {
        self.interface.control_out(ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: DFU_ABORT,
            value: 0,
            index: self.interface.interface_number() as u16,
            data: &[],
        }).await.into_result().map_err(|e| Error::USB("Abort to idle".into(), e.into()))?;

        let s = self.get_status(0).await?;
        if s.state != u8::from(&State::DfuIdle) {
            return Err(Error::InvalidState(s, State::DfuIdle));
        }
        Ok(())
    }

    /// Download file to device using raw mode.
    /// If length is None it will read to file end.
    pub async fn download_raw(
        &mut self,
        file: &mut File,
        address: u32,
        mut length: u32,
    ) -> Result<(), Error> {
        self.erase_pages(address, length).await?;
        self.abort_to_idle().await?;
        self.status_wait_for(0, Some(State::DfuIdle)).await?;
        let mut transaction = 2;
        let mut xfer;
        while length != 0 {
            if length >= self.dfu_descriptor.transfer_size as u32 {
                xfer = self.dfu_descriptor.transfer_size;
                length -= self.dfu_descriptor.transfer_size as u32;
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
            file.read_exact(&mut buf)?;
            self.dfuse_download(Vec::from(DfuseCommand::SetAddress(address)), 0).await?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle)).await?;
            self.dfuse_download(buf, transaction).await?;
            self.status_wait_for(100, Some(State::DfuDownloadBusy)).await?;
            self.status_wait_for(100, Some(State::DfuDownloadIdle)).await?;
            transaction += 1;
        }
        self.abort_to_idle().await?;
        Ok(())
    }

    async fn dfuse_download(&mut self, buf: Vec<u8>, transaction: u16) -> Result<(), Error> {
        let res = self.interface.control_out(ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: DFU_DNLOAD,
            value: transaction,
            index: self.interface.interface_number() as u16,
            data: &buf,
        }).await.into_result();

        match res
        {
            Err(e) => {
                match e {
                    nusb::transfer::TransferError::Stall => {
                        log::warn!("stalled on transaction {}", transaction);
                        self.abort_to_idle().await?;
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        Ok(())
                    }
                    _ => Err(Error::USB("Dfuse download".into(), e.into())),
                }
            }
            Ok(_) => Ok(()),
        }
    }


    pub fn memory_layout(&self) -> &MemoryLayout {
        &self.mem_layout
    }

    async fn dfuse_upload(&mut self, transaction: u16, xfer: u16) -> Result<Vec<u8>, Error> {
        let res = self.interface.control_in(ControlIn {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: DFU_UPLOAD,
            value: transaction,
            index: self.interface.interface_number() as u16,
            length: xfer,
        }).await.into_result();

        match res
        {
            Err(e) => Err(Error::USB("Dfuse upload".into(), e.into())),
            Ok(buf) => Ok(buf),
        }
    }

    pub fn usb(&mut self) -> &mut nusb::Device {
        &mut self.usb
    }
}
