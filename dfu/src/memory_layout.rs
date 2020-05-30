use crate::error::Error;
use std::fmt;
use std::str::FromStr;

pub struct MemoryLayout {
    start_address: u32,
    pages: Vec<u32>,
}

impl FromStr for MemoryLayout {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let s = &s.replace("0x", "");
        let mut sp = s.split("/");
        let start_address = sp.nth(1).ok_or(Error::MemoryLayout(s.into()))?;
        let start_address =
            u32::from_str_radix(&start_address, 16).map_err(|_| Error::MemoryLayout(s.into()))?;
        let mut pages = Vec::new();
        for p in sp
            .next()
            .ok_or(Error::MemoryLayout(format!("Missing pages in {}", s)))?
            .split(",")
        {
            let mut keyval = p.split("*");
            let page_count: u32 = keyval
                .next()
                .ok_or(Error::MemoryLayout(p.into()))?
                .parse()
                .map_err(|_| Error::MemoryLayout(p.into()))?;
            let valprefix = keyval.next().ok_or(Error::MemoryLayout(p.into()))?;
            let value = valprefix.trim_matches(char::is_alphabetic);
            let prefix = valprefix.trim_matches(char::is_numeric);
            let mut value: u32 = value
                .parse()
                .map_err(|_| Error::MemoryLayout(value.into()))?;
            match &prefix[0..1] {
                "K" => value *= 1024,
                "M" => value *= 1024 * 1024,
                _ => {
                    return Err(Error::MemoryLayout(format!("Invalid prefix {}", prefix)));
                }
            }
            for _ in 0..page_count {
                pages.push(value);
            }
        }
        Ok(Self {
            start_address,
            pages,
        })
    }
}

impl fmt::Display for MemoryLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Show memory layout:")?;
        writeln!(f)?;
        let mut address = self.start_address;
        for (i, p) in self.pages.iter().enumerate() {
            writeln!(f, "{}: Start: 0x{:08X} Size: {} bytes", i, address, p);
            address += p;
        }

        write!(f, "")
    }
}

mod tests {
    #[test]
    fn test_memory_from() {
        use super::MemoryLayout;
        use std::str::FromStr;
        assert_eq!(true, MemoryLayout::from_str("/").is_err());
        let m = MemoryLayout::from_str("/0x08008000");
        assert_eq!(true, m.is_err());

        let m = MemoryLayout::from_str("/0x08001000/02*16K");
        assert_eq!(true, m.is_ok());
        let m = m.unwrap();
        assert_eq!(0x0800_1000, m.start_address);
        assert_eq!(2, m.pages.len());
        assert_eq!(&16384, m.pages.iter().nth(0).unwrap_or(&0));
        assert_eq!(&16384, m.pages.iter().nth(1).unwrap_or(&0));

        let m = MemoryLayout::from_str("/0x08010000/02*16K,01*64K");
        assert_eq!(true, m.is_ok());
        let m = m.unwrap();
        assert_eq!(0x0801_0000, m.start_address);
        assert_eq!(3, m.pages.len());
        assert_eq!(&16384, m.pages.iter().nth(0).unwrap_or(&0));
        assert_eq!(&16384, m.pages.iter().nth(1).unwrap_or(&0));
        assert_eq!(&65536, m.pages.iter().nth(2).unwrap_or(&0));
    }
}
