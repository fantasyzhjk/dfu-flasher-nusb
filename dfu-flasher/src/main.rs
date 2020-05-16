use std::fmt;
use structopt::StructOpt;
use usbapi::{ControlTransfer, UsbCore, UsbEnumerate};
enum Error {
    DeviceNotFound(String),
    Argument(String),
    InvalidControlResponse(String),
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
#[allow(dead_code)]
const DFU_DNLOAD: u8 = 1;
#[allow(dead_code)]
const DFU_UPLOAD: u8 = 2;
const DFU_GET_STATUS: u8 = 3;
#[allow(dead_code)]
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
        let buf = Vec::with_capacity(6);
        let ctl = ControlTransfer::new(
            ENDPOINT_IN | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_GET_STATUS,
            0,
            interface,
            buf,
            0,
        );
        let data = usb
            .control(ctl)
            .map_err(|e| Error::USBNix("Control transfer".into(), e))?;

        println!("{:X?}", data);
        let mut data = data.iter();
        if data.len() != 6 {
            return Err(Error::InvalidControlResponse("Status".into()));
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

struct Dfu {
    usb: UsbCore,
    timeout: u32,
}

impl Dfu {
    pub fn from_bus_address(bus: u8, address: u8) -> Result<Self, Error> {
        let mut usb =
            UsbCore::from_bus_address(bus, address).map_err(|e| Error::USB("open".into(), e))?;
        println!("{}", usb.get_descriptor_string(1));
        let timeout = 3000;
        Ok(Self { usb, timeout })
    }

    pub fn get_status(&mut self, interface: u16) -> Result<Status, Error> {
        Status::get(&mut self.usb, interface)
    }

    pub fn clear_status(&mut self, interface: u16) -> Result<(), Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::no_data(
            ENDPOINT_OUT | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_CLRSTATUS,
            0,
            interface,
            self.timeout,
        );
        println!("bailed");
        let _ = self
            .usb
            .control(ctl)
            .map_err(|e| Error::USBNix("Control transfer".into(), e))?;

        println!("bailed not");
        Ok(())
    }

    pub fn detach(&mut self, interface: u16) -> Result<(), Error> {
        use usbapi::os::linux::usbfs::*;
        let ctl = ControlTransfer::no_data(
            ENDPOINT_OUT | REQUEST_TYPE_CLASS | RECIPIENT_INTERFACE,
            DFU_DETACH,
            0,
            interface,
            self.timeout,
        );
        let _ = self
            .usb
            .control(ctl)
            .map_err(|e| Error::USBNix("Control transfer".into(), e))?;

        Ok(())
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
    let _ = dfu.clear_status(0)?;
    println!("{}", dfu.get_status(0)?);
    let _ = dfu.detach(0)?;
    println!("{}", dfu.get_status(0)?);

    Ok(())
}

fn main() {
    if let Err(err) = run_main() {
        eprintln!("{}", err);
        std::process::exit(i32::from(err));
    }
}
