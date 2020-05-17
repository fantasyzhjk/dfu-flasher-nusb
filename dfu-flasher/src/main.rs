use std::fmt;
use std::fs::File;
use structopt::StructOpt;
use usbapi::{ControlTransfer, UsbCore, UsbEnumerate};
enum Error {
    DeviceNotFound(String),
    Argument(String),
    InvalidControlResponse(String),
    InvalidState(Status, State),
    USB(String, std::io::Error),
    USBNix(String, nix::Error),
}

impl From<Error> for i32 {
    fn from(err: Error) -> Self {
        use Error::*;
        match err {
            DeviceNotFound(_) => 64,
            Argument(_) => 65,
            USB(_, _) => 66,
            USBNix(_, _) => 67,
            InvalidControlResponse(_) => 68,
            InvalidState(_, _) => 69,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use Error::*;
        match self {
            DeviceNotFound(d) => write!(f, "Device not found: {}", d),
            Argument(d) => write!(f, "Argument {}", d),
            USB(e, io) => write!(f, "USB {} failed cause {}", e, io),
            USBNix(e, io) => write!(f, "USB {} failed cause {}", e, io),
            InvalidControlResponse(w) => write!(f, "Invalid control response on {}", w),
            InvalidState(s, expect) => write!(
                f,
                "Invalid state Get status gave:\n{}\nExpected state: {}",
                s, expect
            ),
        }
    }
}

#[derive(StructOpt)]
struct Args {
    /// vendor_id:product_id example 0470:df00
    #[structopt(short, long)]
    device: Option<String>,
    #[structopt(skip)]
    id_vendor: u16,
    #[structopt(skip)]
    id_product: u16,
    #[structopt(short, long)]
    bus_address: Option<String>,
    #[structopt(skip)]
    bus: u8,
    #[structopt(skip)]
    address: u8,
}

impl Args {
    fn new() -> Result<Self, Error> {
        let mut args = Self::from_args();
        if args.device.is_some() && args.bus_address.is_some() {
            return Err(Error::Argument(
                "Both vendor:product and bus:address cannot be specified at once!".into(),
            ));
        } else if let Some(dp) = &args.device {
            let mut dp = dp.split(':');
            args.id_vendor = u16::from_str_radix(dp.next().unwrap_or(""), 16).unwrap_or(0);
            args.id_product = u16::from_str_radix(dp.next().unwrap_or(""), 16).unwrap_or(0);
            if args.id_vendor == 0 || args.id_product == 0 {
                return Err(Error::Argument("Expect a device:product as hex".into()));
            }
        } else if let Some(dp) = &args.bus_address {
            let mut dp = dp.split(':');
            args.bus = u8::from_str_radix(dp.next().unwrap_or(""), 10).unwrap_or(0);
            args.address = u8::from_str_radix(dp.next().unwrap_or(""), 10).unwrap_or(0);
            if args.bus == 0 || args.address == 0 {
                return Err(Error::Argument("expect bus:address".into()));
            }
        } else {
            return Err(Error::Argument("-b or -d must specified!".into()));
        }

        Ok(args)
    }
}

#[allow(dead_code)]
const DFU_DETACH: u8 = 0;
const DFU_DNLOAD: u8 = 1;
#[allow(dead_code)]
const DFU_UPLOAD: u8 = 2;
const DFU_GET_STATUS: u8 = 3;
const DFU_CLRSTATUS: u8 = 4;
#[allow(dead_code)]
const DFU_GETSTATE: u8 = 5;
#[allow(dead_code)]
const DFU_ABORT: u8 = 6;

enum State {
    AppIdle,
    AppDetach,
    DfuIdle,
    DfuDownloadSync,
    DfuDownloadBusy,
    DfuDownloadIdle,
    DfuManifestSync,
    DfuManifest,
    DfuManifestWaitReset,
    DfuUploadIdle,
    DfuError,
    Unknown,
}

impl From<State> for u8 {
    fn from(state: State) -> u8 {
        use crate::State::*;
        match state {
            AppIdle => 0,
            AppDetach => 1,
            DfuIdle => 2,
            DfuDownloadSync => 3,
            DfuDownloadBusy => 4,
            DfuDownloadIdle => 5,
            DfuManifestSync => 6,
            DfuManifest => 7,
            DfuManifestWaitReset => 8,
            DfuUploadIdle => 9,
            DfuError => 10,
            Unknown => 255,
        }
    }
}

impl From<u8> for State {
    fn from(state: u8) -> State {
        use crate::State::*;
        match state {
            0 => AppIdle,
            1 => AppDetach,
            2 => DfuIdle,
            3 => DfuDownloadSync,
            4 => DfuDownloadBusy,
            5 => DfuDownloadIdle,
            6 => DfuManifestSync,
            7 => DfuManifest,
            8 => DfuManifestWaitReset,
            9 => DfuUploadIdle,
            10 => DfuError,
            _ => Unknown,
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use crate::State::*;
        match self {
            AppIdle => write!(f, "App Idle"),
            AppDetach => write!(f, "App detach"),
            DfuIdle => write!(f, "Dfu Idle"),
            DfuDownloadSync => write!(f, "Dfu download sync"),
            DfuDownloadBusy => write!(f, "Dfu download busy"),
            DfuDownloadIdle => write!(f, "Dfu download idle"),
            DfuManifestSync => write!(f, "Dfu manifest sync"),
            DfuManifest => write!(f, "Dfu manifest"),
            DfuManifestWaitReset => write!(f, "Dfu manifest wait reset"),
            DfuUploadIdle => write!(f, "Dfu Upload idle"),
            DfuError => write!(f, "Dfu error"),
            Unknown => write!(f, "Unknown state"),
        }
    }
}

#[derive(Default)]
struct Status {
    status: u8,
    poll_timeout: usize,
    state: u8,
    string_index: u8,
}
impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let _ = writeln!(f, "Status: {}", self.status).is_ok();
        let _ = writeln!(f, "poll_timeout: {}", self.poll_timeout).is_ok();
        let _ = writeln!(f, "State: {}", State::from(self.state)).is_ok();
        write!(f, "string_index: {}", self.string_index)
    }
}

impl Status {
    pub fn get(usb: &mut UsbCore, interface: u16) -> Result<Self, Error> {
        let mut s = Self::default();
        use usbapi::os::linux::usbfs::*;
        let buf = vec![0 as u8; 6];
        let ctl = ControlTransfer::new(
            ENDPOINT_IN | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_GET_STATUS,
            0,
            interface,
            Some(buf),
            0,
        );
        let data = usb
            .control(ctl)
            .map_err(|e| Error::USBNix("Control transfer: DFU_GET_STATUS".into(), e))?;

        println!("{:X?}", data);
        let mut data = data.iter();
        if data.len() != 6 {
            return Err(Error::InvalidControlResponse(format!(
                "Status length was {}",
                data.len()
            )));
        }
        s.status = *(data.next().unwrap_or(&(0 as u8)));
        s.poll_timeout = ((*(data.next().unwrap_or(&(0 as u8))) as usize) << 16) as usize;
        s.poll_timeout |= ((*(data.next().unwrap_or(&(0 as u8))) as usize) << 8) as usize;
        s.poll_timeout |= (*(data.next().unwrap_or(&(0 as u8)))) as usize;
        s.state = *(data.next().unwrap_or(&(0 as u8)));
        s.string_index = *(data.next().unwrap_or(&(0 as u8)));
        Ok(s)
    }
}

pub enum DfuseCommand {
    SetAddress(u32),
    ErasePage(u32),
    MassErase,
    ReadUnprotected,
}

impl From<DfuseCommand> for Vec<u8> {
    fn from(command: DfuseCommand) -> Vec<u8> {
        use crate::DfuseCommand::*;
        let mut buf = Vec::new();
        let address = match command {
            SetAddress(address) => {
                buf.push(0x21 as u8);
                Some(address)
            }
            ErasePage(address) => {
                buf.push(0x41 as u8);
                Some(address)
            }
            MassErase => {
                buf.push(0x41 as u8);
                None
            }
            ReadUnprotected => {
                buf.push(0x92 as u8);
                None
            }
        };

        if let Some(address) = address {
            buf.push((address & 0xFF) as u8);
            buf.push((address >> 8) as u8);
            buf.push((address >> 16) as u8);
            buf.push((address >> 24) as u8);
        }

        buf
    }
}

mod tests {
    use crate::DfuseCommand;
    #[test]
    fn test_dfuse_command() {
        let vec = Vec::from(DfuseCommand::MassErase);
        assert!(true, vec.len() == 1);
        assert!(true, vec[0] == 0x41);

        let vec = Vec::from(DfuseCommand::ReadUnprotected);
        assert!(true, vec.len() == 1);
        assert!(true, vec[0] == 0x92);

        let vec = Vec::from(DfuseCommand::SetAddress(0x08100000));
        assert!(true, vec.len() == 5);
        assert!(true, vec[0] == 0x21);
        assert!(true, vec[1] == 0x00);
        assert!(true, vec[2] == 0x00);
        assert!(true, vec[3] == 0x01);
        assert!(true, vec[4] == 0x08);

        let vec = Vec::from(DfuseCommand::ErasePage(0x08100200));
        assert!(true, vec.len() == 5);
        assert!(true, vec[0] == 0x41);
        assert!(true, vec[1] == 0x00);
        assert!(true, vec[2] == 0x02);
        assert!(true, vec[3] == 0x01);
        assert!(true, vec[4] == 0x08);
    }
}

struct Dfu {
    usb: UsbCore,
    timeout: u32,
    interface: u16,
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
        println!("{}", usb.get_descriptor_string(1));
        let timeout = 3000;
        Ok(Self {
            usb,
            timeout,
            interface: 0,
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
                            eprintln!("try again");
                            std::thread::sleep(std::time::Duration::from_millis(3000));
                            continue;
                        }
                    }
                } else if let Error::InvalidControlResponse(_) = e {
                    eprintln!("try again inv");
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
            .map_err(|e| Error::USBNix("Control transfer".into(), e))?;

        Ok(())
    }

    fn wait_manifest(&mut self, mut retries: u8) -> Result<(), Error> {
        retries += 1;
        let mut s = self.get_status(100)?;
        while retries > 0 {
            if s.state == u8::from(State::DfuManifest) {
                return Ok(());
            }
            retries -= 1;
            s = self.get_status(100)?;
        }
        Err(Error::InvalidState(s, State::DfuManifest))
    }

    pub fn reset_stm32(&mut self, address: u32) -> Result<Status, Error> {
        self.dfuse_download(Some(Vec::from(DfuseCommand::SetAddress(address))), 0)?;
        self.wait_manifest(0)?;
        self.dfuse_download(None, 2)?;
        self.get_status(100)
    }

    pub fn dfuse_do_upload(&mut self, xfer_size: isize, file: &File) -> Result<(), Error> {
        panic!("not implemented");
    }

    pub fn dfuse_do_dnload(&mut self, xfer_size: usize, file: &File) -> Result<(), Error> {
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
                eprintln!("dl stalled on {:X?}", ctl);
                std::thread::sleep(std::time::Duration::from_millis(10));
                Ok(())
            }
            Err(e) => Err(Error::USBNix("Dfuse command failed".into(), e)),
            Ok(_) => Ok(()),
        }
    }
}

fn run_main() -> Result<(), Error> {
    let args = Args::new()?;
    /*
    let mut e = UsbEnumerate::new();
    e.enumerate()
        .map_err(|e| Error::USB("enumerate".into(), e))?;
    let mut dev = e.devices().iter().filter(|(_bus, d)| {
        if d.device.id_product == args.id_product {
            return true;
        }
        false
    });
    let (_bus, dev) = dev
        .next()
        .ok_or_else(|| Error::DeviceNotFound(args.device.clone()))?;
    */
    println!("{}:{}", args.bus, args.address);
    let mut dfu = Dfu::from_bus_address(args.bus, args.address)?;
    println!("{}", dfu.get_status(0)?);
    let s = dfu.reset_stm32(0x0800_0000)?;
    println!("{}", s);

    Ok(())
}

fn main() {
    if let Err(err) = run_main() {
        eprintln!("{}", err);
        std::process::exit(i32::from(err));
    }
}
