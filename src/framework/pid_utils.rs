// Copyright 2024-2025, shadow3aaa
//
// This file is part of fas-rs.
//
// fas-rs is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version.
//
// fas-rs is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more
// details.
//
// You should have received a copy of the GNU General Public License along
// with fas-rs. If not, see <https://www.gnu.org/licenses/>.

use std::{fs, io::Read};

use stringzilla::sz;

use crate::framework::Result;

pub fn get_process_name(pid: i32) -> Result<String> {
    let cmdline = format!("/proc/{pid}/cmdline");
    let mut cmdline = fs::File::open(cmdline)?;
    let mut buffer = [0u8; 128];
    let _ = cmdline.read(&mut buffer)?;

    let pos = sz::find(buffer, b":");
    if let Some(sub) = pos {
        let buffer = &buffer[..sub];
        let buffer = String::from_utf8_lossy(buffer).into();
        return Ok(buffer);
    }

    let pos = sz::find(buffer, b"\0");
    let buffer = pos.map_or(&buffer[..], |pos| &buffer[..pos]);

    let buffer = String::from_utf8_lossy(buffer).into();
    Ok(buffer)
}
