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

fn parse_hex(src: &str) -> Result<u32, std::num::ParseIntError> {
    if let Some(idx) = src.find("0x") {
        return u32::from_str_radix(&src[idx + 2..], 16);
    }
    src.parse()
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
    #[structopt(short = "s", long, default_value = "0x08000000", parse(try_from_str = parse_hex))]
    address: u32,
    #[structopt(long)]
    mass_erase: bool,
    #[structopt(short, long)]
    reset_stm32: bool,
    /// Read firmware into <file>
    #[structopt(short = "U", long)]
    upload: Option<PathBuf>,
    #[structopt(skip)]
    bus: u8,
    #[structopt(skip)]
    device: u8,
    #[structopt(skip)]
    length: u32,
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
    let mut dfu = Dfu::from_bus_address(args.bus, args.device)?;
    println!("{}", dfu.get_status(0)?);
    let supported_cmds = dfu.dfuse_get_commands()?;
    dfu.status_wait_for(0, Some(State::DfuIdle))?;
    println!("Supported commands:");
    for cmd in supported_cmds {
        println!("{}", cmd);
    }
    dfu.status_wait_for(0, Some(State::DfuIdle))?;
    if let Some(file) = args.upload {
        dfu.dfu_upload(&mut File::create(file)?, args.address, 0xFFFF)?;
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
