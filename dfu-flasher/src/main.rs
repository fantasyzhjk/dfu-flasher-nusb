use dfu::core::Dfu;
use dfu::error::Error;
use dfu::status::State;
use env_logger;
use std::fmt;
use std::fs::OpenOptions;
use std::path::PathBuf;
use structopt::StructOpt;
use usbapi::UsbEnumerate;

fn parse_int(src: &str) -> Result<u32, std::num::ParseIntError> {
    let src = src.replace("_", "");
    if let Some(idx) = src.find("0x") {
        return u32::from_str_radix(&src[idx + 2..], 16);
    }
    src.parse()
}

fn parse_address_and_length_as_some(
    dfuse_address: &str,
) -> Result<(u32, Option<u32>), std::num::ParseIntError> {
    let address;
    let mut sp = dfuse_address.split(":");
    let a = sp.next().unwrap_or("0x0800_0000");
    address = parse_int(a)?;
    let mut length = if let Some(s) = sp.next() {
        Some(parse_int(&s)?)
    } else {
        None
    };
    Ok((address, length))
}

fn parse_address_and_length(address: &str) -> Result<(u32, u32), std::num::ParseIntError> {
    let a = parse_address_and_length_as_some(address)?;
    Ok((a.0, a.1.unwrap_or(0)))
}

fn parse_address_and_pages(dfuse_address: &str) -> Result<(u32, u8), std::num::ParseIntError> {
    let address;
    let length;
    let mut sp = dfuse_address.split(":");
    let a = sp.next().unwrap_or("0x0800_0000");
    address = parse_int(a)?;
    length = parse_int(sp.next().unwrap_or("0"))?;
    if length > 255 {
        panic!("Pages must be less than 256")
    }
    Ok((address, length as u8))
}

mod tests {
    #[test]
    fn test_parse_int() {
        use crate::*;
        assert_eq!(Ok(0x0010_0000), parse_int("0x00100000"));
        assert_eq!(Ok(10), parse_int("10"));
        assert_eq!(Ok(0x00B0_0000), parse_int("0x00B0_0000"));
        assert_eq!(true, parse_int("0x00Z0_0000").is_err());
    }

    #[test]
    fn test_parse_address_and_length() {
        use crate::*;
        assert_eq!(
            true,
            parse_address_and_length("0xFF00_0000")
                .map(|(a, l)| {
                    assert_eq!(0xFF00_0000, a);
                    assert_eq!(0, l);
                })
                .is_ok()
        );
        assert_eq!(
            true,
            parse_address_and_length("0xFF00_0000:1024")
                .map(|(a, l)| {
                    assert_eq!(0xFF00_0000, a);
                    assert_eq!(1024, l);
                })
                .is_ok()
        );
        assert_eq!(true, parse_address_and_length("0xFF00_0000:0x1000").is_ok());
        assert_eq!(
            true,
            parse_address_and_length("0xZZ00_0000:0x1000").is_err()
        );
    }
    #[test]
    fn test_parse_address_and_length_as_some() {
        use crate::*;
        assert_eq!(
            true,
            parse_address_and_length_as_some("0xFF00_0000")
                .map(|(a, l)| {
                    assert_eq!(0xFF00_0000, a);
                    assert_eq!(None, l);
                })
                .is_ok()
        );
        assert_eq!(
            true,
            parse_address_and_length_as_some("0xFF00_0000:1024")
                .map(|(a, l)| {
                    assert_eq!(0xFF00_0000, a);
                    assert_eq!(Some(1024), l);
                })
                .is_ok()
        );
        assert_eq!(true, parse_address_and_length("0xFF00_0000:0x1000").is_ok());
        assert_eq!(
            true,
            parse_address_and_length("0xZZ00_0000:0x1000").is_err()
        );
    }
}

#[derive(StructOpt, PartialEq)]
struct STMResetArgs {
    #[structopt(short = "s", long, default_value = "0x08000000", parse(try_from_str=parse_int))]
    address: u32,
}

#[derive(StructOpt, PartialEq)]
struct EraseArgs {
    /// start_address:num_pages
    #[structopt(short = "s", long, parse(try_from_str=parse_address_and_length))]
    address: (u32, u32),
}

#[derive(StructOpt, PartialEq)]
struct VWFlashArgs {
    /// start address[:length]
    #[structopt(short = "s", long, default_value = "0x08000000", parse(try_from_str=parse_address_and_length_as_some))]
    address: (u32, Option<u32>),
    /// Read firmware into <file>
    #[structopt(short = "f", long)]
    file_name: PathBuf,
}

#[derive(StructOpt, PartialEq)]
struct ReadFlashArgs {
    /// start address[:length]
    #[structopt(short = "s", long, default_value = "0x08000000", parse(try_from_str=parse_address_and_length))]
    address: (u32, u32),
    /// Read firmware into <file>
    #[structopt(short = "f", long)]
    file_name: PathBuf,
    #[structopt(short = "F", long)]
    overwrite: bool,
}

#[derive(StructOpt, PartialEq)]
enum Action {
    SupportedCommands,
    Reset(STMResetArgs),
    EraseAll,
    Erase(EraseArgs),
    Read(ReadFlashArgs),
    Write(VWFlashArgs),
    Verify(VWFlashArgs),
    Detach,
    SetAddress(STMResetArgs),
    MemoryLayout,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use crate::Action::*;
        match self {
            SupportedCommands => write!(f, "List supported commands"),
            Reset(a) => write!(f, "Reset STM32 vector start address: 0x{:04X}", a.address),
            EraseAll => write!(f, "Erase all"),
            Erase(a) => write!(
                f,
                "Erase area start address: 0x{:04X} number of pages: {}.",
                a.address.0, a.address.1
            ),
            Read(a) => write!(
                f,
                "Read flash from start address: 0x{:04X} length: {} bytes and save to file: '{:?}'",
                a.address.0, a.address.1, a.file_name
            ),
            Write(a) => write!(
                f,
                "Write file: '{:?}' to flash at start address: 0x{:04X} length: {:?} bytes.",
                a.file_name, a.address.0, a.address.1
            ),
            Verify(a) => write!(
                f,
                "Read flash from start address: 0x{:04X} length: {:?} bytes and verify using file '{:?}'",
                a.address.0, a.address.1, a.file_name
            ),
            SetAddress(a) => write!(f, "Set address 0x{:04X}", a.address),
            Detach => write!(f, "Detach"),
            MemoryLayout => write!(f, "Memory layout"),
        }
    }
}

#[derive(StructOpt)]
struct Args {
    /// vendor_id:product_id example 0470:df00
    #[structopt(short, long)]
    dev: Option<String>,
    #[structopt(skip)]
    id_vendor: u16,
    #[structopt(skip)]
    id_product: u16,
    #[structopt(short, long)]
    bus_device: Option<String>,
    /// Specify the DFU interface
    #[structopt(short, long, default_value = "0")]
    intf: u32,
    /// Specify Alt setting of the DFU interface by number
    #[structopt(short, long, default_value = "0")]
    alt: u32,
    #[structopt(skip)]
    bus: u8,
    #[structopt(skip)]
    device: u8,

    #[structopt(subcommand)]
    action: Action,
}

impl Args {
    fn new() -> Result<Self, Error> {
        let mut args = Self::from_args();
        if args.dev.is_some() && args.bus_device.is_some() {
            return Err(Error::Argument(
                "Both vendor:product and bus:address cannot be specified at once!".into(),
            ));
        } else if let Some(dp) = &args.dev {
            let mut dp = dp.split(':');
            args.id_vendor = u16::from_str_radix(dp.next().unwrap_or(""), 16).unwrap_or(0);
            args.id_product = u16::from_str_radix(dp.next().unwrap_or(""), 16).unwrap_or(0);
            if args.id_vendor == 0 || args.id_product == 0 {
                return Err(Error::Argument("Expect a device:product as hex".into()));
            }
        } else if let Some(dp) = &args.bus_device {
            let mut dp = dp.split(':');
            args.bus = u8::from_str_radix(dp.next().unwrap_or(""), 10).unwrap_or(0);
            args.device = u8::from_str_radix(dp.next().unwrap_or(""), 10).unwrap_or(0);
            if args.bus == 0 || args.device == 0 {
                return Err(Error::Argument("expect bus:device".into()));
            }
        } else {
            let mut e = UsbEnumerate::new();
            e.enumerate()?;
            let mut msg =
                format!("Missing --bus-device or --dev! List of possible USB devices:\n\n");
            for (bus, dev) in e
                .devices()
                .iter()
                .filter(|(_, dev)| dev.device.id_product == 0xdf11)
            {
                let dev = &dev.device;
                msg += &format!(
                    "--bus-device {} or -d {:04X}:{:04X}\n",
                    bus.replace("-", ":"),
                    dev.id_vendor,
                    dev.id_product,
                );
            }
            return Err(Error::Argument(msg));
        }

        Ok(args)
    }
}

fn run_main() -> Result<(), Error> {
    let args = Args::new()?;
    let mut dfu = Dfu::from_bus_device(args.bus, args.device, args.intf, args.alt)?;
    dfu.status_wait_for(0, Some(State::DfuIdle))?;
    log::info!("Execute action: {}", args.action);
    match args.action {
        Action::SupportedCommands => {
            let supported_cmds = dfu.dfuse_get_commands()?;
            println!("Supported commands:");
            for cmd in supported_cmds {
                println!("{}", cmd);
            }
            Ok(())
        }
        Action::Reset(a) => dfu.reset_stm32(a.address),
        Action::Read(a) => dfu.upload(
            &mut OpenOptions::new()
                .write(true)
                .create(a.overwrite)
                .truncate(a.overwrite)
                .create_new(!a.overwrite)
                .open(a.file_name)?,
            a.address.0,
            a.address.1,
        ),
        Action::Write(a) => dfu.download_raw(
            &mut OpenOptions::new().read(true).open(a.file_name)?,
            a.address.0,
            a.address.1,
        ),

        Action::Verify(a) => dfu.verify(
            &mut OpenOptions::new().read(true).open(a.file_name)?,
            a.address.0,
            a.address.1,
        ),
        Action::EraseAll => dfu.mass_erase(),
        Action::Erase(a) => dfu.erase_pages(a.address.0, a.address.1),
        Action::Detach => dfu.detach(),
        Action::SetAddress(a) => dfu.set_address(a.address),
        Action::MemoryLayout => {
            println!("{}", dfu.memory_layout()?);
            Ok(())
        }
    }
}

fn main() {
    let env = env_logger::Env::default().filter_or("DFU_FLASHER_LOG", "debug");
    env_logger::init_from_env(env);
    if let Err(err) = run_main() {
        log::error!("{}", err);
        std::process::exit(i32::from(err));
    }
}
