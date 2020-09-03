// See https://github.com/m4b/goblin/blob/master/src/pe/symbol.rs for details.

use crate::ByteOrder;
use crate::demangle::SymbolData;
use crate::parser::*;
use crate::ParseError;

use std::ops::Range;
use std::convert::TryInto;

const PE_POINTER_OFFSET: usize = 0x3c;
const COFF_SYMBOL_SIZE: usize = 18;
const IMAGE_SYM_CLASS_EXTERNAL: u8 = 2;
const IMAGE_SYM_DTYPE_SHIFT: usize = 4;
const IMAGE_SYM_DTYPE_FUNCTION: u16 = 2;
const SIZEOF_PE_MAGIC: usize = 4;
const SIZEOF_COFF_HEADER: usize = 20;

#[derive(Debug, Copy, Clone)]
pub struct PeHeader {
    machine: u16,
    number_of_sections: u16,
    time_date_stamp: u32,
    pointer_to_symbol_table: u32,
    number_of_symbols: u32,
    size_of_optional_header: u16,
    characteristics: u16,
}
#[derive(Debug, Copy, Clone)]
pub struct Section<'a> {
    name: &'a str,
    virtual_size: u32,
    size_of_raw_data: u32,
    pointer_to_raw_data: u32,
    index: usize
}

impl Section <'_> {
    pub fn range(&self) -> Result<Range<usize>, ParseError> {
        let start: usize = self.pointer_to_raw_data.try_into()?;
        let end: usize = start.checked_add(self.size_of_raw_data.try_into()?).ok_or(ParseError::MalformedInput)?;
        Ok(start..end)
    }
}

#[derive(Debug, Clone)]
pub struct Pe<'a> {
    data: &'a [u8],
    header: PeHeader,
    sections: Vec<Section<'a>>,
}

fn parse_pe_header(s: &mut Stream) -> Result<PeHeader, UnexpectedEof> {
    s.skip::<u32>()?; // magic
    Ok(PeHeader {
        machine: s.read(),
        number_of_sections: s.read(),
        time_date_stamp: s.read(),
        pointer_to_symbol_table: s.read(),
        number_of_symbols: s.read(),
        size_of_optional_header: s.read(),
        characteristics: s.read(),
    })
}

pub fn parse(data: &[u8]) -> Result<Pe, ParseError> {
    let mut s = Stream::new_at(data, PE_POINTER_OFFSET, ByteOrder::LittleEndian)?;
    let pe_pointer = s.read::<u32>() as usize;

    let mut s = Stream::new_at(data, pe_pointer, ByteOrder::LittleEndian)?;
    let header = parse_pe_header(&mut s)?;

    let sections_offset = //TODO: harden
          pe_pointer
        + SIZEOF_PE_MAGIC
        + SIZEOF_COFF_HEADER
        + header.size_of_optional_header as usize;

    // Won't OOM because number_of_sections is a u16
    let mut sections = Vec::with_capacity(header.number_of_sections.into());
    let mut s = Stream::new_at(data, sections_offset, ByteOrder::LittleEndian)?;
    for i in 0..header.number_of_sections {
        let name = s.read_bytes(8);
        let virtual_size: u32 = s.read();
        s.skip::<u32>()?; // virtual_address
        let size_of_raw_data: u32 = s.read();
        let pointer_to_raw_data: u32 = s.read();
        s.skip_len(16)?; // other data

        let len = name.iter().position(|c| *c == 0).unwrap_or(8);
        // ignore sections with non-UTF8 names since the spec says they must be UTF-8
        if let Ok(name_str) = std::str::from_utf8(&name[0..len]) {
            sections.push(Section{
                name: name_str,
                virtual_size: virtual_size,
                size_of_raw_data: size_of_raw_data,
                pointer_to_raw_data: pointer_to_raw_data,
                index: i.into(),
            })
        }
    }
    Ok(Pe {
        data: data,
        header: header,
        sections: sections,
    })
}

impl Pe<'_> {
    pub fn header(&self) -> PeHeader {
        self.header.clone()
    }

    pub fn sections(&self) -> Vec<Section> {
        self.sections.clone()
    }

    pub fn section_with_name(&self, section_name: &str) -> Option<Section> {
        self.sections.iter().find(|x| {
            x.name == section_name
        }).cloned()
    }

    pub fn symbols(&self) -> Result<(Vec<SymbolData>, u64), ParseError> {
        let number_of_symbols = self.header.number_of_symbols as usize;
        let mut symbols = Vec::with_capacity(number_of_symbols);

        let text_section = self.section_with_name(".text").unwrap();
        let text_section_size = text_section.size_of_raw_data;
        let text_section_index = text_section.index;
    
        // Add the .text section size, which will be used
        // to calculate the size of the last symbol.
        symbols.push(SymbolData {
            name: crate::demangle::SymbolName::demangle(".text"),
            address: text_section_size as u64,
            size: 0,
        });
    
        let mut s = Stream::new_at(self.data, self.header.pointer_to_symbol_table as usize, ByteOrder::LittleEndian).unwrap();
        let symbols_data = s.read_bytes(number_of_symbols * COFF_SYMBOL_SIZE);
        let string_table_offset = s.offset();
    
        let mut s = Stream::new(symbols_data, ByteOrder::LittleEndian);
        while !s.at_end() {
            let name = s.read_bytes(8);
            let value: u32 = s.read();
            let section_number: i16 = s.read();
            let kind: u16 = s.read();
            let storage_class: u8 = s.read();
            let number_of_aux_symbols: u8 = s.read();
            s.skip_len(number_of_aux_symbols as usize * COFF_SYMBOL_SIZE)?;
    
            if (kind >> IMAGE_SYM_DTYPE_SHIFT) != IMAGE_SYM_DTYPE_FUNCTION {
                continue;
            }
    
            if storage_class != IMAGE_SYM_CLASS_EXTERNAL {
                continue;
            }
    
            // `section_number` starts from 1.
            if section_number - 1 != text_section_index as i16 {
                continue;
            }
    
            let name = if !name.starts_with(&[0, 0, 0, 0]) {
                let len = name.iter().position(|c| *c == 0).unwrap_or(8);
                std::str::from_utf8(&name[0..len]).ok()
            } else {
                let mut s2 = Stream::new(&name[4..], ByteOrder::LittleEndian);
                let name_offset: u32 = s2.read();
                parse_null_string(self.data, string_table_offset + name_offset as usize)
            };
    
            if let Some(s) = name {
                symbols.push(SymbolData {
                    name: crate::demangle::SymbolName::demangle(s),
                    address: value as u64,
                    size: 0,
                });
            }
        }
    
        // To find symbol sizes, we have to sort them by address.
        symbols.sort_by_key(|v| v.address);
    
        // PE format doesn't store the symbols size,
        // so we have to calculate it by subtracting an address of the next symbol
        // from the current.
        for i in 1..symbols.len() {
            let curr = symbols[i].address;
            let next_sym = symbols[i..].iter().skip_while(|s| s.address == curr).next();
            if let Some(next_sym) = next_sym {
                symbols[i].size = next_sym.address - curr;
            }
        }
    
        // Remove the last symbol, which is `.text` section size.
        symbols.pop();
    
        Ok((symbols, text_section_size as u64))
    }
}