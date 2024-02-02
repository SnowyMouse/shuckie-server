extern crate shuckie_server;

use std::time::Duration;
use shuckie_server::server::*;

pub const GAMEBOY_APPROX_NS_PER_FRAME: u64 = 1000000000 * ((1024 * 1024 * 2) / 125257647);

fn address_resolver(range: &AddressRange) -> bool {
    let end = match range.address.checked_add(range.size) {
        Some(n) => n,
        None => {
            println!("Overflow detected - range requested too high; can't let you do that :(");
            return false
        }
    };
    let accepted = end < 0x16000;
    println!("Range request for [0x{:08X}..0x{:08X}]: {accepted}", range.address, end);
    accepted
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        println!("Usage: {} <tcp-ip:port> <udp-ip:port>", args[0]);
        return
    }

    let mut server = Server::new(&args[1], &args[2]).unwrap();
    server.set_address_validation_callback(Some(AddressValidationCallback::Rust(address_resolver)));
    server.start_thread().unwrap();
    handle_server(&mut server);
}

fn handle_server(server: &mut Server) {
    let mut buffer = vec![0u8; 0x10000];

    let mut next_frame = std::time::Instant::now();
    loop {
        let now = std::time::Instant::now();
        if now < next_frame {
            continue
        }
        next_frame = now + Duration::from_nanos(GAMEBOY_APPROX_NS_PER_FRAME);

        server.poll_streams(|r| {
            match r {
                QueuedRequest::Write(w) => {
                    let bytes = w.bytes.lock().unwrap();
                    println!("Received write request: 0x{:08X} of size 0x{:08X}", w.address, bytes.len());
                    buffer[w.address as usize .. w.address as usize + bytes.len()].copy_from_slice(bytes.as_slice());
                },
                QueuedRequest::Read(s) => {
                    let address = s.get_address() as usize;
                    let range = address .. (address + s.get_buffer().len());
                    let input = &buffer[range.clone()];
                    let output = &mut s.get_buffer();
                    output.clone_from_slice(input);
                    s.send_buffer();
                }
            }
        })
    }
}
