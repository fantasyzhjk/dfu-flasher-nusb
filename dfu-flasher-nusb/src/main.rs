use dfu_nusb::core::Dfu;
use dfu_nusb::error::Error;
use dfu_nusb::status::State;
use log::info;
use pretty_hex::PrettyHex;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use structopt::StructOpt;

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
    let mut sp = dfuse_address.split(':');
    let a = sp.next().unwrap_or("0x0800_0000");
    let address = parse_int(a)?;
    let length = if let Some(s) = sp.next() {
        Some(parse_int(s)?)
    } else {
        None
    };
    Ok((address, length))
}

fn parse_address_and_length(address: &str) -> Result<(u32, u32), std::num::ParseIntError> {
    let a = parse_address_and_length_as_some(address)?;
    Ok((a.0, a.1.unwrap_or(0)))
}

mod tests {
    #[test]
    fn test_parse_int() {
        use crate::*;
        assert_eq!(Ok(0x0010_0000), parse_int("0x00100000"));
        assert_eq!(Ok(10), parse_int("10"));
        assert_eq!(Ok(0x00B0_0000), parse_int("0x00B0_0000"));
        assert!(parse_int("0x00Z0_0000").is_err());
    }

    #[test]
    fn test_parse_address_and_length() {
        use crate::*;
        assert!(parse_address_and_length("0xFF00_0000")
            .map(|(a, l)| {
                assert_eq!(0xFF00_0000, a);
                assert_eq!(0, l);
            })
            .is_ok());
        assert!(parse_address_and_length("0xFF00_0000:1024")
            .map(|(a, l)| {
                assert_eq!(0xFF00_0000, a);
                assert_eq!(1024, l);
            })
            .is_ok());
        assert!(parse_address_and_length("0xFF00_0000:0x1000").is_ok());
        assert!(parse_address_and_length("0xZZ00_0000:0x1000").is_err());
    }
    #[test]
    fn test_parse_address_and_length_as_some() {
        use crate::*;
        assert!(parse_address_and_length_as_some("0xFF00_0000")
            .map(|(a, l)| {
                assert_eq!(0xFF00_0000, a);
                assert_eq!(None, l);
            })
            .is_ok());
        assert!(parse_address_and_length_as_some("0xFF00_0000:1024")
            .map(|(a, l)| {
                assert_eq!(0xFF00_0000, a);
                assert_eq!(Some(1024), l);
            })
            .is_ok());
        assert!(parse_address_and_length("0xFF00_0000:0x1000").is_ok());
        assert!(parse_address_and_length("0xZZ00_0000:0x1000").is_err());
    }
}

#[derive(StructOpt, PartialEq)]
struct STMResetArgs {
    #[structopt(short = "s", long, default_value = "0x08000000", parse(try_from_str=parse_int))]
    address: u32,
}

#[derive(StructOpt, PartialEq)]
struct AddressArgs {
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
    Erase(AddressArgs),
    Read(ReadFlashArgs),
    Write(VWFlashArgs),
    Verify(VWFlashArgs),
    Detach,
    SetAddress(STMResetArgs),
    MemoryLayout,
    ReadAddress(AddressArgs),
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
                "Read flash from start address: 0x{:08X} length: {:?} bytes and verify using file '{:?}'",
                a.address.0, a.address.1, a.file_name
            ),
            SetAddress(a) => write!(f, "Set address 0x{:08X}", a.address),
            Detach => write!(f, "Detach"),
            MemoryLayout => write!(f, "Memory layout"),
            ReadAddress(a) => write!(f, "Read address 0x{:08X} length: {} bytes", a.address.0, a.address.1),
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
    intf: u8,
    /// Specify Alt setting of the DFU interface by number
    #[structopt(short, long, default_value = "0")]
    alt: u8,
    #[structopt(skip)]
    bus: u8,
    #[structopt(skip)]
    device: u8,
    #[structopt(subcommand)]
    action: Action,
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,
}

impl Args {
    fn new() -> Result<Self, Error> {
        let mut args = Self::from_args();
        env_logger_init("dfu-flasher", args.verbose);
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
            args.bus = dp.next().unwrap_or("").parse::<u8>().unwrap_or(0);
            args.device = dp.next().unwrap_or("").parse::<u8>().unwrap_or(0);
            if args.bus == 0 || args.device == 0 {
                return Err(Error::Argument("expect bus:device".into()));
            }
        } else {
            let e = nusb::list_devices()?;
            let mut msg =
                String::from("Missing --bus-device or --dev! List of possible USB devices:\n\n");
            for (bus, dev) in e.filter(|dev| dev.product_id() == 0xdf11).map(|dev| {
                (
                    format!("{:04X}:{:04X}", dev.bus_number(), dev.device_address()),
                    dev,
                )
            }) {
                msg += &format!(
                    "--bus-device {} or -d {:04X}:{:04X}\n",
                    bus,
                    dev.vendor_id(),
                    dev.product_id(),
                );
            }
            return Err(Error::Argument(msg));
        }

        Ok(args)
    }
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

async fn run_main() -> Result<(), Error> {
    let args = Args::new()?;
    let mut dfu = if args.id_vendor != 0 && args.id_product != 0 {
        Dfu::from_vid_pid(args.id_vendor, args.id_product, args.intf, args.alt).await?
    } else {
        Dfu::from_bus_device(args.bus, args.device, args.intf, args.alt).await?
    };
    dfu.status_wait_for(0, Some(State::DfuIdle)).await?;
    log::info!("Execute action: {}", args.action);
    match args.action {
        Action::SupportedCommands => {
            let supported_cmds = dfu.dfuse_get_commands().await?;
            println!("Supported commands:");
            for cmd in supported_cmds {
                println!("{}", cmd);
            }
            Ok(())
        }
        Action::Reset(a) => dfu.reset_stm32(a.address).await,
        Action::Read(a) => dfu.upload(
            &mut OpenOptions::new()
                .write(true)
                .create(a.overwrite)
                .truncate(a.overwrite)
                .create_new(!a.overwrite)
                .open(a.file_name)?,
            a.address.0,
            a.address.1,
        ).await,
        Action::Write(a) => {
            let f = &mut OpenOptions::new().read(true).open(a.file_name)?;
            let len = get_length_from_file(f, a.address.1).unwrap();
            dfu.download_raw(f, a.address.0, len).await
        }
        Action::Verify(a) => {
            let f = &mut OpenOptions::new().read(true).open(a.file_name)?;
            let len = get_length_from_file(f, a.address.1).unwrap();
            dfu.verify(f, a.address.0, len).await?;
            info!("Verify done");
            Ok(())
        }
        Action::EraseAll => dfu.mass_erase().await,
        Action::Erase(a) => dfu.erase_pages(a.address.0, a.address.1).await,
        Action::Detach => dfu.detach().await,
        Action::ReadAddress(a) => {
            let mut buf = vec![0; a.address.1 as usize];
            let len = dfu.read_flash_to_slice(a.address.0, &mut buf).await?;
            // let mut address = a.address.0;
            //print!("0x{:08X} ", address);
            //address += 16;
            println!("{:?}", buf[0..len].hex_dump());
            Ok(())
        }
        Action::SetAddress(a) => dfu.set_address(a.address).await,
        Action::MemoryLayout => {
            dfu.memory_layout().pages().iter().for_each(|p| {
                println!("Start: 0x{:08X} Size: {} bytes", p.address, p.size)
            });
            Ok(())
        }
    }
}

fn env_logger_init(_appname: &str, verbose: usize) {
    use env_logger::Builder;
    use log::LevelFilter;
    match verbose {
        0 => Builder::from_default_env()
            .filter(None, LevelFilter::Info)
            .format_timestamp_millis()
            .init(),
        1 => Builder::from_default_env()
            .filter(None, LevelFilter::Debug)
            .init(),
        _ => Builder::from_default_env()
            .filter(None, LevelFilter::Trace)
            .init(),
    }
}

#[tokio::main]
async fn main() {
    if let Err(err) = run_main().await {
        log::error!("{}", err);
        std::process::exit(i32::from(err));
    }
}
