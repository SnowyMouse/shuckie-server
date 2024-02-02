use super::*;

use std::io::Result as IOResult;
type StrResult<T> = std::result::Result<T, &'static str>;

type CommandFn = fn (args: &[&str], peer: &mut PeerImpl) -> IOResult<()>;

pub struct Command {
    pub name: &'static str,
    pub function: CommandFn,
    pub arguments: (usize, usize),
    pub description: &'static str,
    pub usage: &'static str,
}

pub const COMMANDS: &'static [Command] = &[
    Command { name: "ECHO", function: echo, arguments: (1, 1024), description: "Prints what you sent.", usage: "<arg1> [arg2 ...]" },
    Command { name: "HELP", function: help, arguments: (1, 1), description: "Get help for the given command.", usage: "<command>" },
    Command { name: "LIST_COMMANDS", function: list_commands, arguments: (0, 0), description: "List all commands.", usage: "" },
    Command { name: "LIST_STREAMS", function: list_streams, arguments: (0, 0), description: "List all currently streamed addresses and lengths. Fails if not streaming.", usage: "" },
    Command { name: "STOP_STREAMING", function: stop_streaming, arguments: (0, 1), description: "Stop streaming at the requested address, or no argument to stop all streaming. Fails if not streaming that address.", usage: "[address]" },
    Command { name: "STREAM_MEMORY", function: stream_memory, arguments: (2, 2), description: "Stream memory from the given address to the subscribed address via UDP. Fails if already streaming that address.", usage: "<address> <length>" },
    Command { name: "SUBSCRIBE", function: subscribe, arguments: (1, 1), description: "Subscribe with the given UDP port (IP must match your TCP IP). Fails if already subscribed.", usage: "<port>" },
    Command { name: "UNSUBSCRIBE", function: unsubscribe, arguments: (0, 0), description: "Unsubscribe without clearing the list of streamed addresses. Must be done before re-subscribing. Fails if not subscribed.", usage: "" },
    Command { name: "WRITE_MEMORY", function: write_memory, arguments: (2, usize::MAX), description: "Writes the memory to the given address, but note that the actual write may be slightly delayed if the emulator thread is busy.", usage: "<address> <byte1> [byte2 ...]" }
];

fn help(args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    let command = match COMMANDS.binary_search_by(|probe| probe.name.cmp(args[0])) {
        Ok(n) => &COMMANDS[n],
        Err(_) => return writeln!(peer.stream, "ERR No such command `{}`", args[0])
    };

    writeln!(peer.stream, "OK {} {}{}- {}",
        command.name,
        if command.usage.is_empty() { "" } else { command.usage },
        if command.usage.is_empty() { "" } else { " " },
        command.description
    )
}

fn list_commands(_args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    let mut iter = COMMANDS.iter();
    write!(peer.stream, "OK {}", iter.next().unwrap().name)?;
    for i in iter {
        write!(peer.stream, ", {}", i.name)?;
    }
    writeln!(peer.stream)
}

fn echo(args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    let mut iter = args.iter();
    write!(peer.stream, "OK `{}`", iter.next().unwrap())?;
    for i in iter {
        write!(peer.stream, ", `{}`", i)?;
    }
    writeln!(peer.stream)
}

fn subscribe(args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    if peer.subscribe_addr.is_some() {
        return writeln!(peer.stream, "ERR Already subscribed");
    }

    let port: u16 = match args[0].parse() {
        Ok(n) => n,
        Err(_) => return writeln!(peer.stream, "ERR Invalid port")
    };

    let mut addr = peer.addr;
    addr.set_port(port);
    peer.subscribe_addr = Some(addr);

    writeln!(peer.stream, "OK All UDP packets will be sent to {addr}")
}

fn unsubscribe(_args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    if peer.subscribe_addr.is_some() {
        peer.subscribe_addr = None;
        writeln!(peer.stream, "OK Unsubscribed")
    }
    else {
        writeln!(peer.stream, "ERR Not subscribed")
    }
}

fn validate_address_range(peer: &mut PeerImpl, address: u64, size: u64) -> IOResult<bool> {
    if !peer.address_validation_callback.call(&AddressRange { address, size }) {
        writeln!(peer.stream, "ERR Address range given is not valid for this emulator")?;
        Ok(false)
    }
    else {
        Ok(true)
    }
}

fn write_memory(args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    let address = match parse_str_to_u64(args[0]) {
        Some(n) => n,
        None => return writeln!(peer.stream, "ERR Invalid address")
    };

    let bytes = args.len() - 1;
    let mut data: Vec<u8> = Vec::with_capacity(bytes);
    if !validate_address_range(peer, address, bytes as u64)? {
        return Ok(())
    }

    let input = args[1..].iter().map(|f| parse_str_to_u64(*f));
    for i in input {
        let byte = match i {
            Some(n) if n <= u8::MAX as u64 => n as u8,
            _ => return writeln!(peer.stream, "ERR Invalid data")
        };
        data.push(byte);
    }

    writeln!(peer.stream, "OK Writing {} byte{} to 0x{:X}", data.len(), if data.len() == 1 { "" } else { "s" }, address)?;
    peer.queued_writes.push(QueuedWrite { address, bytes: Arc::new(Mutex::new(data)) });
    Ok(())
}

fn parse_str_to_u64(what: &str) -> Option<u64> {
    match what.parse::<u64>() {
        Ok(n) => return Some(n),
        _ => ()
    }

    if !what.starts_with("0x") {
        return None
    }

    let hex_str = &what[2..];
    if hex_str.len() > (u64::BITS as usize) / 8 * 2 {
        return None
    }

    let mut number = 0u64;
    for h in hex_str.chars() {
        let digit = h.to_ascii_lowercase().to_digit(16)? as u64;
        number = (number << 4) | digit;
    }

    Some(number)
}

fn stream_memory(args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    let address = parse_str_to_u64(args[0]);
    let size = parse_str_to_u64(args[1]);

    if address == None || size == None || size == Some(0) {
        return writeln!(peer.stream, "ERR Invalid address and/or size");
    }

    let address = address.unwrap();
    let size = size.unwrap();
    let address_end = match address.checked_add(size) {
        Some(n) => n,
        None => return writeln!(peer.stream, "ERR Size would make address overflow")
    };

    if peer.streams.iter().any(|p| {
        let start = p.address;
        let end = start + p.buffer.len() as u64;
        address_end > start && address < end
    }) {
        return writeln!(peer.stream, "ERR Already streaming somewhere at 0x{:X}-0x{:X}", address, address_end);
    }

    let data: Vec<u8> = Vec::with_capacity(args.len() - 1);
    if !validate_address_range(peer, address, data.len() as u64)? {
        return Ok(())
    }

    peer.streams.push(Stream::new(address, size));
    writeln!(peer.stream, "OK Now streaming at 0x{:X}", address)
}

fn list_streams(_args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    if peer.streams.is_empty() {
        return writeln!(peer.stream, "ERR No streams");
    }

    let write_addr = |stream: &mut TcpStream, address: u64, size: usize| write!(stream, "0x{address:X}[{size}]");
    let mut iter = peer.streams.iter();
    write!(peer.stream, "OK ")?;

    let first = iter.next().unwrap();
    write_addr(&mut peer.stream, first.address, first.buffer.len())?;
    for i in iter {
        write!(peer.stream, ", ")?;
        write_addr(&mut peer.stream, i.address, i.buffer.len())?;
    }
    writeln!(peer.stream)
}

fn stop_streaming(args: &[&str], peer: &mut PeerImpl) -> IOResult<()> {
    if peer.streams.is_empty() {
        return writeln!(peer.stream, "ERR No streams");
    }

    if args.is_empty() {
        peer.streams.clear();
        return writeln!(peer.stream, "OK Streams cleared");
    }

    let address = match parse_str_to_u64(args[0]) {
        Some(n) => n,
        None => return writeln!(peer.stream, "ERR Invalid address")
    };

    let size = peer.streams.len();
    peer.streams.retain(|f| f.address != address);
    let deleted = size - peer.streams.len();

    if deleted == 0 {
        return writeln!(peer.stream, "ERR Not streaming at 0x{:X}", address);
    }

    writeln!(peer.stream, "OK No longer streaming at 0x{:X}", address)
}

pub struct CommandSplitter<'a> {
    data: &'a str
}

impl<'a> CommandSplitter<'a> {
    pub fn new(data: &'a str) -> Self {
        CommandSplitter {
            data
        }
    }
}

impl<'a> Iterator for CommandSplitter<'a> {
    type Item = StrResult<String>;
    fn next(&mut self) -> Option<Self::Item> {
        let mut end: Option<usize> = None;
        let mut in_quotes = false;
        let mut escaped = false;

        let mut result: Option<String> = None;

        for (offset, character) in self.data.char_indices() {
            if !escaped {
                match character {
                    '\\' => escaped = true,
                    ' ' => {
                        if result == None {
                            continue
                        }
                        if !in_quotes {
                            end = Some(offset);
                            break;
                        }
                    },
                    '"' => {
                        if in_quotes {
                            end = Some(offset);
                            break;
                        }
                        else if result != None {
                            return Some(Err("unexpected quotes"));
                        }
                    }
                    _ => ()
                }
            }
            else {
                escaped = false;
            }

            // Initialize if not already
            if result == None {
                result = Some(String::with_capacity(self.data.len() - offset));
                if character == '"' {
                    in_quotes = true;
                    continue;
                }
            }

            // Don't add the escape character
            if escaped {
                continue;
            }

            result.as_mut().unwrap().push(character);
        }

        if result == None && end == None {
            self.data = "";
            return None
        }

        if end == None {
            if in_quotes {
                return Some(Err("unterminated quotes"));
            }
            end = Some(self.data.len());
        }

        let mut end = end.unwrap();

        if in_quotes {
            end = end + 1;
        }

        self.data = &self.data[end..];
        Some(Ok(result.unwrap()))
    }
}

#[cfg(test)]
mod test;
