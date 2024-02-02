extern crate shuckie_server;

use std::ffi::{c_char, CStr};
use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::ptr;
use std::sync::Mutex;
use shuckie_server::server::*;

static LAST_ERROR: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn set_last_error(new_error: String) {
    let mut e = LAST_ERROR.lock().unwrap();
    e.clear();
    e.write_all(new_error.as_bytes()).unwrap();
    e.push(0);
}

#[no_mangle]
pub extern "C" fn shuckie_server_get_last_error() -> *const c_char {
    LAST_ERROR.lock().unwrap().as_ptr() as *const _
}


unsafe fn c_str_to_tcp_ip(ip_port: *const c_char) -> Result<SocketAddr, String> {
    if ip_port.is_null() {
        return Err("IP:Port given was null!".to_owned());
    }

    let ip_port = match CStr::from_ptr(ip_port).to_str() {
        Ok(n) => n,
        Err(_) => return Err("IP:Port given not valid UTF-8".to_owned())
    };

    ip_port
        .to_socket_addrs()
        .map_err(|_| format!("{ip_port} does not correspond to a valid socket address"))?
        .next()
        .ok_or_else(|| format!("{ip_port} does not correspond to a valid socket address"))
}

#[no_mangle]
pub unsafe extern "C" fn shuckie_server_new(
    udp_bind: *const c_char,
    tcp_bind: *const c_char,
) -> *mut Server {
    let result = (|| -> Result<Server, String> {
        let tcp_bind = c_str_to_tcp_ip(tcp_bind)?;
        let udp_bind = c_str_to_tcp_ip(udp_bind)?;
        Server::new(tcp_bind, udp_bind)
            .map_err(|e| e.to_string())
    })();

    match result {
        Ok(n) => Box::into_raw(Box::new(n)),
        Err(e) => {
            set_last_error(e);
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn shuckie_server_free(
    server: *mut Server
) {
    if !server.is_null() {
        drop(Box::from_raw(server))
    }
}

#[no_mangle]
pub extern "C" fn shuckie_server_start_thread(
    server: &mut Server
) -> bool {
    match server.start_thread() {
        Ok(()) => true,
        Err(e) => {
            set_last_error(e.to_string());
            false
        }
    }
}

#[no_mangle]
pub extern "C" fn shuckie_server_stop_thread(
    server: &mut Server
) -> bool {
    match server.stop_thread() {
        Ok(()) => true,
        Err(e) => {
            set_last_error(e.to_string());
            false
        }
    }
}

#[no_mangle]
pub extern "C" fn shuckie_server_set_address_validation_callback(
    server: &mut Server,
    address_validation_callback: Option<extern "C" fn(u64, u64) -> bool>
) {
    let callback = match address_validation_callback {
        Some(n) => n,
        None => {
            server.set_address_validation_callback(None);
            return;
        }
    };
    server.set_address_validation_callback(Some(AddressValidationCallback::Extern(callback)));
}

#[no_mangle]
pub extern "C" fn shuckie_server_poll_streams(
    server: &mut Server,
    user_data: *mut (),
    callback: extern "C" fn(&QueuedRequest, *mut ())
) {
    server.poll_streams(|request| callback(&request, user_data));
}

#[repr(C)]
pub enum QueuedRequestType {
    Read = 0,
    Write = 1
}

#[no_mangle]
pub extern "C" fn shuckie_server_queued_request_type(request: &QueuedRequest) -> QueuedRequestType {
    match request {
        QueuedRequest::Read(_) => QueuedRequestType::Read,
        QueuedRequest::Write(_) => QueuedRequestType::Write
    }
}

#[no_mangle]
pub extern "C" fn shuckie_server_get_read_data(request: &mut QueuedRequest, address: &mut u64, buffer: &mut *mut u8, buffer_length: &mut u64) {
    match request {
        QueuedRequest::Read(r) => {
            *address = r.get_address();
            *buffer = r.get_buffer().as_mut_ptr();
            *buffer_length = r.get_buffer().len() as u64;
        },
        _ => panic!("not a read request")
    }
}

#[no_mangle]
pub extern "C" fn shuckie_server_send_buffer(request: &mut QueuedRequest) {
    match request {
        QueuedRequest::Read(r) => r.send_buffer(),
        _ => panic!("not a read request")
    }
}

#[no_mangle]
pub extern "C" fn shuckie_server_get_write_data(request: &mut QueuedRequest, address: &mut u64, buffer: &mut *const u8, buffer_length: &mut u64) {
    match request {
        QueuedRequest::Write(w) => {
            *address = w.address;

            let data = w.bytes.lock().unwrap();
            *buffer = data.as_ptr();
            *buffer_length = data.len() as u64;
        },
        _ => panic!("not a write request")
    }
}
