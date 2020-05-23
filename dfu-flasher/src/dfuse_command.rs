use std::convert::TryFrom;
use std::fmt;
#[derive(Debug)]
pub enum DfuseCommand {
    SetAddress(u32),
    ErasePage(u32),
    MassErase,
    ReadUnprotected,
}

impl TryFrom<u8> for DfuseCommand {
    type Error = crate::error::Error;
    fn try_from(cmd: u8) -> Result<Self, crate::error::Error> {
        use crate::DfuseCommand::*;
        match cmd {
            0x21 => Ok(SetAddress(0)),
            0x41 => Ok(MassErase),
            0x92 => Ok(ReadUnprotected),
            b => Err(crate::error::Error::UnknownCommandByte(b)),
        }
    }
}

impl From<DfuseCommand> for Vec<u8> {
    fn from(command: DfuseCommand) -> Vec<u8> {
        use crate::DfuseCommand::*;
        let mut buf: Vec<u8> = Vec::new();
        let address = match command {
            SetAddress(address) => {
                buf.push(0x21);
                Some(address)
            }
            ErasePage(address) => {
                buf.push(0x41);
                Some(address)
            }
            MassErase => {
                buf.push(0x41);
                None
            }
            ReadUnprotected => {
                buf.push(0x92);
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

impl fmt::Display for DfuseCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use crate::DfuseCommand::*;
        match self {
            SetAddress(_) => write!(f, "Set address"),
            ErasePage(_) | MassErase => write!(f, "Page/Mass erase"),
            ReadUnprotected => write!(f, "Read unprotected"),
        }
    }
}

mod tests {
    use crate::DfuseCommand;
    #[test]
    fn test_dfuse_command() {
        let vec = Vec::from(DfuseCommand::MassErase);
        assert_eq!(1, vec.len());
        assert_eq!(&vec![0x41], &vec);

        let vec = Vec::from(DfuseCommand::ReadUnprotected);
        assert_eq!(1, vec.len());
        assert_eq!(&vec![0x92], &vec);

        let vec = Vec::from(DfuseCommand::SetAddress(0x0801_0000));
        assert_eq!(5, vec.len());
        assert_eq!(&vec![0x21, 0x00, 0x00, 0x01, 0x08], &vec);

        let vec = Vec::from(DfuseCommand::ErasePage(0x0801_0200));
        assert_eq!(5, vec.len());
        assert_eq!(&vec![0x41, 0x00, 0x02, 0x01, 0x08], &vec);
    }
}
