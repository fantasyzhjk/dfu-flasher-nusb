use crate::error::Error;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Serialize)]
pub struct Page {
    pub(crate) address: u32,
    pub(crate) size: u32,
}

#[derive(Debug, Serialize)]
pub struct MemoryLayout {
    pages: Vec<Page>,
}

impl FromStr for MemoryLayout {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let s = &s.replace("0x", "");
        let mut sp = s.split('/');
        let address = sp.nth(1).ok_or_else(|| Error::MemoryLayout(s.into()))?;
        let mut address =
            u32::from_str_radix(&address, 16).map_err(|_| Error::MemoryLayout(s.into()))?;
        let mut pages = Vec::new();
        for p in sp
            .next()
            .ok_or_else(|| Error::MemoryLayout(format!("Missing pages in {}", s)))?
            .split(',')
        {
            let mut keyval = p.split('*');
            let page_count: u32 = keyval
                .next()
                .ok_or_else(|| Error::MemoryLayout(p.into()))?
                .parse()
                .map_err(|_| Error::MemoryLayout(p.into()))?;
            let valprefix = keyval.next().ok_or_else(|| Error::MemoryLayout(p.into()))?;
            let size = valprefix.trim_matches(char::is_alphabetic);
            let prefix = valprefix.trim_matches(char::is_numeric);
            let mut size: u32 = size.parse().map_err(|_| Error::MemoryLayout(size.into()))?;
            match &prefix[0..1] {
                "K" => size *= 1024,
                "M" => size *= 1024 * 1024,
                _ => {
                    return Err(Error::MemoryLayout(format!("Invalid prefix {}", prefix)));
                }
            }
            for _ in 0..page_count {
                pages.push(Page { address, size });
                address += size;
            }
        }
        Ok(Self { pages })
    }
}

impl fmt::Display for MemoryLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Show memory layout:")?;
        writeln!(f)?;
        for (i, p) in self.pages.iter().enumerate() {
            writeln!(
                f,
                "{}: Start: 0x{:08X} Size: {} bytes",
                i, p.address, p.size
            )?;
        }

        write!(f, "")
    }
}

impl MemoryLayout {
    pub fn pages(&self) -> &Vec<Page> {
        &self.pages
    }

    /// Return num_pages in region specified
    pub fn num_pages(&self, mut address: u32, length: u32) -> Result<usize, Error> {
        let mut pages = 0;
        let end = address + length;
        while address < end {
            let p = self.address(address)?;
            address += p.size;
            pages += 1;
        }
        Ok(pages)
    }

    pub fn address(&self, address: u32) -> Result<Page, Error> {
        for p in &self.pages {
            if address >= p.address && address < p.address + p.size {
                return Ok(Page {
                    address: p.address,
                    size: p.size,
                });
            }
        }
        Err(Error::Address(address))
    }
}

mod tests {
    #[test]
    fn test_memory_address() {
        use super::MemoryLayout;
        use std::str::FromStr;
        // 0: 0x0801_0000 to 0801_3FFF 16K
        // 1: 0x0801_4000 to 0801_7FFF 16K
        // 2: 0x0801_8000 to 0802_7FFF 64K
        let m = MemoryLayout::from_str("/0x08010000/02*16K,01*64K").unwrap();
        assert_eq!(true, m.address(0x0800_0000).is_err());
        let p = m.address(0x0801_0100).unwrap();
        assert_eq!(0x0801_0000, p.address);
        assert_eq!(0x4000, p.size);
        let p = m.address(0x0801_4000).unwrap();
        assert_eq!(0x0801_4000, p.address);
        assert_eq!(0x4000, p.size);

        let p = m.address(0x0801_8001).unwrap();
        assert_eq!(0x0801_8000, p.address);
        assert_eq!(0x10000, p.size);
        assert_eq!(true, m.address(0x0802_7FFF).is_ok());

        assert_eq!(true, m.address(0x0802_8000).is_err());
    }
    #[test]
    fn test_memory_num_pages() {
        use super::MemoryLayout;
        use std::str::FromStr;
        // 0: 0x0801_0000 to 0801_3FFF 16K
        // 1: 0x0801_4000 to 0801_7FFF 16K
        // 2: 0x0801_8000 to 0802_7FFF 64K
        let m = MemoryLayout::from_str("/0x08010000/02*16K,01*64K").unwrap();
        assert_eq!(true, m.num_pages(0x0800_0000, 0xFFFF).is_err());
        let n = m.num_pages(0x0801_0000, 0xFFFF).unwrap();
        assert_eq!(3, n);

        let n = m.num_pages(0x0801_0000, 0x2000).unwrap();
        assert_eq!(1, n);

        let n = m.num_pages(0x0801_0000, 0x4000).unwrap();
        assert_eq!(1, n);

        let n = m.num_pages(0x0801_0000, 0x4001).unwrap();
        assert_eq!(2, n);

        let n = m.num_pages(0x0801_4000, 0x2000).unwrap();
        assert_eq!(1, n);

        let n = m.num_pages(0x0801_4000, 0x8000).unwrap();
        assert_eq!(2, n);
    }
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
        let p = m.pages();
        assert_eq!(2, p.len());
        assert_eq!(16384, p.iter().nth(0).unwrap().size);
        assert_eq!(16384, p.iter().nth(1).unwrap().size);

        let m = MemoryLayout::from_str("/0x08010000/02*16K,01*64K");
        assert_eq!(true, m.is_ok());
        let m = m.unwrap();
        let p = m.pages();
        assert_eq!(3, p.len());
        assert_eq!(16384, p.iter().nth(0).unwrap().size);
        assert_eq!(16384, p.iter().nth(1).unwrap().size);
        assert_eq!(65536, p.iter().nth(2).unwrap().size);
    }
}
