use std::net::{ToSocketAddrs, UdpSocket, TcpListener, TcpStream, SocketAddr};
use std::thread::JoinHandle;
use crate::result::*;
use std::io::{Cursor, Read, Write};
use std::sync::{Arc, Mutex};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

mod command;
use command::*;

/// Default maximum peers for a [`Server`] instance.
///
/// Can be overridden with [`Server::set_max_peers`]
pub const DEFAULT_MAX_PEERS: usize = 5;
pub type UserData = *mut ();

#[derive(Clone)]
pub struct Server {
    server: Arc<Mutex<Pin<Box<ServerImpl>>>>
}

struct ServerImpl {
    server_thread: Option<JoinHandle<()>>,
    stopping: bool,

    udp_socket: UdpSocket,
    tcp_listener: TcpListener,
    address_validation_callback: AddressValidationCallback,

    max_peers: usize,
    peers: Vec<Peer>
}

#[derive(Clone)]
pub struct Peer {
    peer: Arc<Mutex<Pin<Box<PeerImpl>>>>
}

const PACKET_SIZE: usize = 1024;
const UDP_FOURCC: u32 = 0x4455587E;
const UDP_PROTOCOL_VERSION: u32 = 1;

#[derive(Copy, Clone, Default, PartialEq)]
#[repr(C)]
pub struct AddressRange {
    pub domain: u64,
    pub address: u64,
    pub size: u64
}

#[derive(Clone)]
pub struct QueuedWrite {
    pub domain: u64,
    pub address: u64,
    pub bytes: Arc<Mutex<Vec<u8>>>
}

pub enum QueuedRequest<'a> {
    Read(&'a mut Stream),
    Write(&'a QueuedWrite)
}

#[derive(Copy, Clone)]
pub enum AddressValidationCallback {
    Rust(fn(&AddressRange) -> bool),
    Extern(extern "C" fn(u64, u64, u64) -> bool)
}
impl AddressValidationCallback {
    pub fn call(&self, range: &AddressRange) -> bool {
        match self {
            Self::Rust(n) => n(range),
            Self::Extern(n) => n(range.domain, range.address, range.size)
        }
    }
}

fn address_validation_callback_no_op(_: &AddressRange) -> bool {
    false
}

pub struct Stream {
    domain: u64,
    address: u64,
    pending: bool,
    buffer: Vec<u8>,
    sequence_index: AtomicU64
}

impl Stream {
    fn new(domain: u64, address: u64, size: u64) -> Self {
        Stream {
            domain,
            address,
            pending: false,
            buffer: vec![0; size.try_into().unwrap()],
            sequence_index: AtomicU64::new(0)
        }
    }

    /// Get the domain to read from.
    pub fn get_domain(&self) -> u64 {
        self.domain
    }

    /// Get the address to read from.
    pub fn get_address(&self) -> u64 {
        self.address
    }

    /// Get the buffer to write data to.
    pub fn get_buffer(&mut self) -> &mut [u8] {
        self.buffer.as_mut_slice()
    }

    /// Send the contents of the buffer.
    pub fn send_buffer(&mut self) {
        self.pending = true
    }

    fn respond(&mut self, socket: &UdpSocket, to: &SocketAddr) {
        fn send_packet(domain: u64, address: u64, data: &[u8], sequence_index: u64, socket: &UdpSocket, to: &SocketAddr) -> bool {
            let packet = [0u8; PACKET_SIZE + 100];
            let mut cursor = Cursor::new(packet);

            cursor.write(&UDP_FOURCC.to_be_bytes()).unwrap();
            cursor.write(&UDP_PROTOCOL_VERSION.to_be_bytes()).unwrap();
            cursor.write(&sequence_index.to_be_bytes()).unwrap();
            cursor.write(&domain.to_be_bytes()).unwrap();
            cursor.write(&address.to_be_bytes()).unwrap();
            cursor.write(&(data.len() as u64).to_be_bytes()).unwrap();
            cursor.write(data).unwrap();

            let size = cursor.position() as usize;
            if let Err(e) = socket.send_to(&cursor.get_ref()[0..size], to) {
                eprintln!("Failed to send {} bytes of address {address:08X} to {to}: {e:?}", data.len());
                false
            }
            else {
                true
            }
        }

        if !self.pending {
            return
        }

        let sequence_index = self.sequence_index.fetch_add(1, Ordering::Relaxed);
        let domain = self.domain;
        let mut address = self.address;
        for i in (0..self.buffer.len()).step_by(PACKET_SIZE) {
            let range = &self.buffer[i..(i + PACKET_SIZE).min(self.buffer.len())];
            if !send_packet(domain, address, range, sequence_index, socket, to) {
                return
            }
            address += range.len() as u64;
        }
        self.pending = false;
    }
}

struct PeerImpl {
    stream: TcpStream,
    buffer: Vec<u8>,
    addr: SocketAddr,
    streams: Vec<Stream>,
    queued_writes: Vec<QueuedWrite>,
    address_validation_callback: AddressValidationCallback,

    subscribe_addr: Option<SocketAddr>
}

impl Server {
    /// Initialize a new server, binding to the given UDP and TCP addresses.
    ///
    /// This also initializes the maximum peers to [`DEFAULT_MAX_PEERS`].
    pub fn new<A: ToSocketAddrs, B: ToSocketAddrs>(udp_addr: A, tcp_addr: B) -> Result<Server> {
        let udp_socket = io(UdpSocket::bind(udp_addr))?;
        io(udp_socket.set_nonblocking(true))?;

        let tcp_listener = io(TcpListener::bind(tcp_addr))?;
        io(tcp_listener.set_nonblocking(true))?;

        Ok(Server {
            server: Arc::new(Mutex::new(Box::pin(ServerImpl {
                udp_socket,
                tcp_listener,
                max_peers: DEFAULT_MAX_PEERS,
                peers: Vec::new(),
                server_thread: None,
                address_validation_callback: AddressValidationCallback::Rust(address_validation_callback_no_op),
                stopping: false
            })))
        })
    }

    /// Set the maximum number of peers.
    pub fn set_max_peers(&mut self, amount: usize) {
        let mut handle = self.server.lock().unwrap();
        handle.max_peers = amount;
    }

    /// Start a server thread.
    ///
    /// Returns `Err` if the thread already exists.
    pub fn start_thread(&mut self) -> Result<()> {
        let handle = self.server.clone();
        let mut server = self.server.lock().unwrap();
        if server.server_thread.is_some() || server.stopping {
            return Err(Error::ThreadAlreadyExists)
        }
        server.server_thread = Some(std::thread::spawn(|| {
            server_thread(handle)
        }));

        Ok(())
    }

    /// Stop the server thread and disconnect all peers.
    ///
    /// Returns `Err` if the server has not started.
    pub fn stop_thread(&mut self) -> Result<()> {
        let mut server = self.server.lock().unwrap();
        let thread = std::mem::take(&mut server.server_thread);
        server.peers.clear();

        if let Some(n) = thread {
            server.stopping = true;
            drop(server);
            n.join().unwrap();
            self.server.lock().unwrap().stopping = false;
            Ok(())
        }
        else {
            Err(Error::NotStarted)
        }
    }

    /// Set the address validation callback.
    ///
    /// By default all addresses are allowed, but you can limit this to a specific range.
    pub fn set_address_validation_callback(&mut self, address_validation_callback: Option<AddressValidationCallback>) {
        self.server.lock().unwrap().address_validation_callback = address_validation_callback.unwrap_or(
            AddressValidationCallback::Rust(address_validation_callback_no_op)
        );
    }

    /// Get all pending streams and handle them
    pub fn poll_streams<T>(&mut self, mut callback: T) where T: FnMut(QueuedRequest) {
        let server = self.server.lock().unwrap();
        for i in &server.peers {
            let mut peer = i.peer.lock().unwrap();
            for w in &peer.queued_writes {
                callback(QueuedRequest::Write(w));
            }
            peer.queued_writes.clear();
            for r in &mut peer.streams {
                callback(QueuedRequest::Read(r));
            }
        }
    }
}

const DEFAULT_BUFFER_CAPACITY: usize = 1024 * 1024;

impl ServerImpl {
    fn accept_client(&mut self, stream: TcpStream, addr: SocketAddr) {
        if self.peers.len() >= self.max_peers {
            dprintln!("Rejecting client {addr} due to peer limit {} exceeded", self.max_peers);
            return
        }

        stream.set_nonblocking(true).unwrap();
        stream.set_nodelay(true).unwrap();

        dprintln!("Accepting client {addr}");
        self.peers.push(Peer {
                peer: Arc::new(Mutex::new(Box::pin(PeerImpl {
                    stream,
                    addr,
                    buffer: Vec::with_capacity(DEFAULT_BUFFER_CAPACITY),
                    subscribe_addr: None,
                    streams: Vec::with_capacity(256),
                    queued_writes: Vec::with_capacity(256),
                    address_validation_callback: AddressValidationCallback::Rust(address_validation_callback_no_op)
                }
            )))
        })
    }
}

impl PeerImpl {
    /// Return true if the peer should be disconnected
    fn handle_command(&mut self) -> std::result::Result<(), &'static str> {
        macro_rules! wrap {
            ($what:expr) => (($what).map_err(|_| "write failure"));
        }

        if self.buffer.is_empty() {
            return Ok(())
        }

        // Parse the command
        let mut command_buffer = Vec::with_capacity(DEFAULT_BUFFER_CAPACITY);
        std::mem::swap(&mut command_buffer, &mut self.buffer);

        let command_full = std::str::from_utf8(command_buffer.as_slice());
        let command_full = match command_full {
            Ok(c) => c,
            Err(_) => {
                dprintln!("Received non-UTF8 command from {}, disconnecting...", self.addr);
                return Err("non-utf8 command sequence received")
            }
        };

        let args_maybe: Vec<std::result::Result<String, &'static str>> = CommandSplitter::new(command_full).collect();
        if args_maybe.is_empty() {
            return Ok(())
        }

        let mut args: Vec<&str> = Vec::with_capacity(args_maybe.len());
        for i in &args_maybe {
            match i {
                Ok(n) => args.push(n.as_str()),
                Err(reason) => {
                    dprintln!("Received improperly formatted command from {}: {reason}", self.addr);
                    return wrap!(writeln!(self.stream, "ERR Failed to parse command: {reason}"));
                }
            }
        }

        let command = &args[0];
        let args = &args[1..];

        // Find the command
        let found = COMMANDS.binary_search_by(|probe| { (*probe.name).cmp(command) });
        match found {
            Ok(n) => {
                let cmd = &COMMANDS[n];
                if cmd.arguments.0 > args.len() || cmd.arguments.1 < args.len() {
                    dprintln!("Received command from {}: {command_full}, but incorrect number of args given; {} outside of [{},{}]", self.addr, args.len(), cmd.arguments.0, cmd.arguments.1);

                    let s = if cmd.arguments.1 == 1 { "" } else { "s" };

                    if cmd.arguments.0 == cmd.arguments.1 {
                        wrap!(writeln!(self.stream, "ERR `{command}` takes exactly {} argument{s}; use `HELP {command}` for usage", cmd.arguments.0))?;
                    }
                    else if cmd.arguments.1 == usize::MAX {
                        wrap!(writeln!(self.stream, "ERR `{command}` takes at least {} argument{s}; use `HELP {command}` for usage", cmd.arguments.0))?;
                    }
                    else {
                        wrap!(writeln!(self.stream, "ERR `{command}` takes {}-{} arguments; use `HELP {command}` for usage", cmd.arguments.0, cmd.arguments.1))?;
                    }
                }
                else {
                    dprintln!("Received command from {}: {command_full}", self.addr);
                    wrap!((COMMANDS[n].function)(args, self))?;
                }
                Ok(())
            },
            Err(_) => {
                dprintln!("Received command from {}: {command_full}; {command} is not a recognized command, however", self.addr);
                wrap!(writeln!(self.stream, "ERR No such command `{command}`; use `LIST_COMMANDS` to list commands"))
            }
        }
    }

    /// Update read streams from the stream command.
    fn update_read_streams(&mut self, socket: &UdpSocket) {
        let address = match self.subscribe_addr.as_ref() {
            Some(n) => n,
            None => return
        };
        for i in &mut self.streams {
            i.respond(socket, address)
        }
    }
}

fn server_thread(what: Arc<Mutex<Pin<Box<ServerImpl>>>>) {
    loop {
        let mut server = what.lock().unwrap();
        if server.stopping {
            return;
        }

        let address_validation_callback = server.address_validation_callback;
        let mut removal: Vec<(usize, &'static str)> = Vec::with_capacity(server.peers.len());
        let socket = server.udp_socket.try_clone().unwrap();

        // Handle input from peers
        for c in 0..server.peers.len() {
            let client = &mut server.peers[c];
            let mut peer = client.peer.lock().unwrap();

            let mut schedule_for_removal = |reason: &'static str| {
                removal.push((c, reason));
            };

            loop {
                let mut byte = [0u8];
                let result = peer.stream.read(&mut byte);

                match result {
                    Ok(0) => {
                        schedule_for_removal("EOF reached; disconnected");
                        break;
                    },
                    Ok(1) => {
                        // Newline hit; handle command
                        if byte[0] == '\n' as u8 || byte[0] == '\r' as u8 {
                            peer.address_validation_callback = address_validation_callback;
                            match peer.handle_command() {
                                Err(n) => schedule_for_removal(n),
                                Ok(_) => ()
                            }
                            break;
                        }

                        // Disconnect if buffer sized exceeded
                        else if peer.buffer.len() == peer.buffer.capacity() {
                            schedule_for_removal("buffer size exceeded");
                            break;
                        }

                        // Append to buffer
                        else {
                            peer.buffer.push(byte[0])
                        }
                    },
                    Ok(_) => unreachable!(),
                    Err(_) => break
                }
            }

            // Send anything if pending
            peer.update_read_streams(&socket);
        }

        // Drop peers that don't exist anymore
        for (index, _reason) in removal.iter().rev() {
            let _addr = server.peers.remove(*index).peer.lock().unwrap().addr;
            dprintln!("Removing peer {_addr}: {_reason}");
        }

        // Accept new peers
        loop {
            let (stream, addr) = match server.tcp_listener.accept() {
                Ok(n) => n,
                Err(_) => break
            };
            server.accept_client(stream, addr);
        }
    }
}
