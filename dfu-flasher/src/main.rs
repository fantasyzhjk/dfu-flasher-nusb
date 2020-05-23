mod dfu_core;
mod dfu_status;
mod dfuse_command;
mod error;
use dfu_core::Dfu;
use dfu_status::State;
use dfuse_command::DfuseCommand;
use error::Error;
use std::fs::File;
use std::path::PathBuf;
use structopt::StructOpt;

fn parse_int(src: &str) -> Result<u32, std::num::ParseIntError> {
    let src = src.replace("_", "");
    if let Some(idx) = src.find("0x") {
        return u32::from_str_radix(&src[idx + 2..], 16);
    }
    src.parse()
}

fn parse_address_and_length(dfuse_address: &str) -> Result<(u32, u32), Error> {
    let address;
    let length;
    let mut sp = dfuse_address.split(":");
    let a = sp.next().unwrap_or("0x0800_0000");
    address = parse_int(a).map_err(|_| {
            Error::Argument(format!(
                "Argument --dfuse-address expects address[:length] as Hex or decimal you passed '{}'.\nExample read/write 1024 bytes to address 0x80000000:\n--dfuse-address 0x0800_0000:1024",
                dfuse_address
            ))
    })?;
    length = parse_int(sp.next().unwrap_or("0")).map_err(|_| {
            Error::Argument(format!(
                "Argument --dfuse-address expects address[:length] as Hex or decimal you passed '{}'.\nExample read/write 1024 bytes to address 0x80000000:\n--dfuse-address 0x0800_0000:1024",
                dfuse_address
            ))
    })?;

    Ok((address, length))
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
    /// Address[:Length]
    #[structopt(short = "s", long, default_value = "0x08000000")]
    dfuse_address: String,
    /// Erase all data on flash
    #[structopt(long)]
    mass_erase: bool,
    #[structopt(short, long)]
    reset_stm32: bool,
    /// Specify the DFU interface
    #[structopt(short, long, default_value = "0")]
    intf: u32,
    /// Specify Alt setting of the DFU interface by number
    #[structopt(short, long, default_value = "0")]
    alt: u32,
    /// Read firmware into <file>
    #[structopt(short = "U", long)]
    upload: Option<PathBuf>,
    #[structopt(skip)]
    bus: u8,
    #[structopt(skip)]
    device: u8,
    #[structopt(skip)]
    address: u32,
    #[structopt(skip)]
    length: u32,
}

impl Args {
    fn new() -> Result<Self, Error> {
        let mut args = Self::from_args();
        let (a, l) = parse_address_and_length(&args.dfuse_address)?;
        args.address = a;
        args.length = l;
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
            return Err(Error::Argument("-b or -d must specified!".into()));
        }

        Ok(args)
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
    println!("{}:{}", args.bus, args.device);
    let mut dfu = Dfu::from_bus_address(args.bus, args.device, args.intf, args.alt)?;
    println!("{}", dfu.get_status(0)?);
    let supported_cmds = dfu.dfuse_get_commands()?;
    dfu.status_wait_for(0, Some(State::DfuIdle))?;
    println!("Supported commands:");
    for cmd in supported_cmds {
        println!("{}", cmd);
    }
    dfu.status_wait_for(0, Some(State::DfuIdle))?;
    if let Some(file) = args.upload {
        dfu.dfu_upload(&mut File::create(file)?, args.address, args.length)?;
    }

    if args.mass_erase {
        dfu.dfuse_mass_erase()?;
    }

    if args.reset_stm32 {
        println!("reset stm {:X}", args.address);
        dfu.abort_to_idle()?;
        dfu.reset_stm32(0)?;
    }

    Ok(())
}

fn main() {
    if let Err(err) = run_main() {
        eprintln!("{}", err);
        std::process::exit(i32::from(err));
    }
}
