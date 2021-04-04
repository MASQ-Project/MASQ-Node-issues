// Copyright (c) 2019-2021, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::command_context::{CommandContext, ContextError};
use crate::command_factory::{CommandFactory, CommandFactoryError};
use crate::command_processor::{CommandProcessor, CommandProcessorFactory};
use crate::commands::commands_common::CommandError::Transmission;
use crate::commands::commands_common::{Command, CommandError};
use crate::communications::broadcast_handler::StreamFactory;
use crate::line_reader::{TerminalEvent, TerminalReal};
use crate::terminal_interface::{
    InterfaceRaw, Terminal, TerminalInterfaceFactory, TerminalWrapper, WriterGeneric,
};
use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use linefeed::memory::MemoryTerminal;
use linefeed::{Interface, ReadResult};
use masq_lib::intentionally_blank;
use masq_lib::test_utils::fake_stream_holder::{ByteArrayWriter, ByteArrayWriterInner};
use masq_lib::ui_gateway::MessageBody;
use std::borrow::BorrowMut;
use std::cell::{Ref, RefCell};
use std::fmt::Arguments;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{io, thread};

#[derive(Default)]
pub struct CommandFactoryMock {
    make_params: Arc<Mutex<Vec<Vec<String>>>>,
    make_results: RefCell<Vec<Result<Box<dyn Command>, CommandFactoryError>>>,
}

impl CommandFactory for CommandFactoryMock {
    fn make(&self, pieces: Vec<String>) -> Result<Box<dyn Command>, CommandFactoryError> {
        self.make_params.lock().unwrap().push(pieces);
        self.make_results.borrow_mut().remove(0)
    }
}

impl CommandFactoryMock {
    pub fn new() -> Self {
        Self {
            make_params: Arc::new(Mutex::new(vec![])),
            make_results: RefCell::new(vec![]),
        }
    }

    pub fn make_params(mut self, params: &Arc<Mutex<Vec<Vec<String>>>>) -> Self {
        self.make_params = params.clone();
        self
    }

    pub fn make_result(self, result: Result<Box<dyn Command>, CommandFactoryError>) -> Self {
        self.make_results.borrow_mut().push(result);
        self
    }
}

pub struct CommandContextMock {
    active_port_results: RefCell<Vec<Option<u16>>>,
    send_params: Arc<Mutex<Vec<MessageBody>>>,
    send_results: RefCell<Vec<Result<(), ContextError>>>,
    transact_params: Arc<Mutex<Vec<(MessageBody, u64)>>>,
    transact_results: RefCell<Vec<Result<MessageBody, ContextError>>>,
    stdout: Box<dyn Write>,
    stdout_arc: Arc<Mutex<ByteArrayWriterInner>>,
    stderr: Box<dyn Write>,
    stderr_arc: Arc<Mutex<ByteArrayWriterInner>>,
}

impl CommandContext for CommandContextMock {
    fn active_port(&self) -> Option<u16> {
        self.active_port_results.borrow_mut().remove(0)
    }

    fn send(&mut self, message: MessageBody) -> Result<(), ContextError> {
        self.send_params.lock().unwrap().push(message);
        self.send_results.borrow_mut().remove(0)
    }

    fn transact(
        &mut self,
        message: MessageBody,
        timeout_millis: u64,
    ) -> Result<MessageBody, ContextError> {
        self.transact_params
            .lock()
            .unwrap()
            .push((message, timeout_millis));
        self.transact_results.borrow_mut().remove(0)
    }

    fn stdin(&mut self) -> &mut dyn Read {
        unimplemented!()
    }

    fn stdout(&mut self) -> &mut dyn Write {
        &mut self.stdout
    }

    fn stderr(&mut self) -> &mut dyn Write {
        &mut self.stderr
    }

    fn close(&mut self) {
        unimplemented!()
    }
}

impl Default for CommandContextMock {
    fn default() -> Self {
        let stdout = ByteArrayWriter::new();
        let stdout_arc = stdout.inner_arc();
        let stderr = ByteArrayWriter::new();
        let stderr_arc = stderr.inner_arc();
        Self {
            active_port_results: RefCell::new(vec![]),
            send_params: Arc::new(Mutex::new(vec![])),
            send_results: RefCell::new(vec![]),
            transact_params: Arc::new(Mutex::new(vec![])),
            transact_results: RefCell::new(vec![]),
            stdout: Box::new(stdout),
            stdout_arc,
            stderr: Box::new(stderr),
            stderr_arc,
        }
    }
}

impl CommandContextMock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_port_result(self, result: Option<u16>) -> Self {
        self.active_port_results.borrow_mut().push(result);
        self
    }

    pub fn send_params(mut self, params: &Arc<Mutex<Vec<MessageBody>>>) -> Self {
        self.send_params = params.clone();
        self
    }

    pub fn send_result(self, result: Result<(), ContextError>) -> Self {
        self.send_results.borrow_mut().push(result);
        self
    }

    pub fn transact_params(mut self, params: &Arc<Mutex<Vec<(MessageBody, u64)>>>) -> Self {
        self.transact_params = params.clone();
        self
    }

    pub fn transact_result(self, result: Result<MessageBody, ContextError>) -> Self {
        self.transact_results.borrow_mut().push(result);
        self
    }

    pub fn stdout_arc(&self) -> Arc<Mutex<ByteArrayWriterInner>> {
        self.stdout_arc.clone()
    }

    pub fn stderr_arc(&self) -> Arc<Mutex<ByteArrayWriterInner>> {
        self.stderr_arc.clone()
    }
}

#[derive(Default)]
pub struct CommandProcessorMock {
    process_params: Arc<Mutex<Vec<Box<dyn Command>>>>,
    process_results: RefCell<Vec<Result<(), CommandError>>>,
    close_params: Arc<Mutex<Vec<()>>>,
    terminal_interface: Vec<TerminalWrapper>,
    terminal_interface_clone_count: Arc<Mutex<usize>>,
    upgrade_terminal_interface_results: Vec<Result<(), String>>,
}

impl CommandProcessor for CommandProcessorMock {
    fn process(&mut self, command: Box<dyn Command>) -> Result<(), CommandError> {
        self.process_params.lock().unwrap().push(command);
        self.process_results.borrow_mut().remove(0)
    }

    fn close(&mut self) {
        self.close_params.lock().unwrap().push(());
    }

    fn upgrade_terminal_interface(&mut self) -> Result<(), String> {
        self.upgrade_terminal_interface_results.remove(0)
    }

    fn clone_terminal_interface(&mut self) -> TerminalWrapper {
        *self.terminal_interface_clone_count.lock().unwrap() += 1;
        self.terminal_interface[0].clone()
    }
}

impl CommandProcessorMock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_terminal_interface(
        mut self,
        terminal_interface_arc_clone: TerminalWrapper,
    ) -> Self {
        self.terminal_interface.push(terminal_interface_arc_clone);
        self
    }
    pub fn insert_terminal_wrapper_shared_counter(mut self, reference: Arc<Mutex<usize>>) -> Self {
        self.terminal_interface_clone_count = reference;
        self
    }

    pub fn process_params(mut self, params: &Arc<Mutex<Vec<Box<dyn Command>>>>) -> Self {
        self.process_params = params.clone();
        self
    }

    pub fn process_result(self, result: Result<(), CommandError>) -> Self {
        self.process_results.borrow_mut().push(result);
        self
    }

    pub fn upgrade_terminal_interface_result(mut self, result: Result<(), String>) -> Self {
        self.upgrade_terminal_interface_results.push(result);
        self
    }

    pub fn close_params(mut self, params: &Arc<Mutex<Vec<()>>>) -> Self {
        self.close_params = params.clone();
        self
    }
}

#[derive(Default)]
pub struct CommandProcessorFactoryMock {
    make_params: Arc<Mutex<Vec<Vec<String>>>>,
    make_results: RefCell<Vec<Result<Box<dyn CommandProcessor>, CommandError>>>,
}

impl CommandProcessorFactory for CommandProcessorFactoryMock {
    fn make(
        &self,
        _broadcast_stream_factory: Box<dyn StreamFactory>,
        args: &[String],
    ) -> Result<Box<dyn CommandProcessor>, CommandError> {
        self.make_params.lock().unwrap().push(args.to_vec());
        self.make_results.borrow_mut().remove(0)
    }
}

impl CommandProcessorFactoryMock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn make_params(mut self, params: &Arc<Mutex<Vec<Vec<String>>>>) -> Self {
        self.make_params = params.clone();
        self
    }

    pub fn make_result(self, result: Result<Box<dyn CommandProcessor>, CommandError>) -> Self {
        self.make_results.borrow_mut().push(result);
        self
    }
}

pub struct MockCommand {
    message: MessageBody,
    execute_results: RefCell<Vec<Result<(), CommandError>>>,
}

impl std::fmt::Debug for MockCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "MockCommand")
    }
}

impl Command for MockCommand {
    fn execute(&self, context: &mut dyn CommandContext) -> Result<(), CommandError> {
        write!(context.stdout(), "MockCommand output").unwrap();
        write!(context.stderr(), "MockCommand error").unwrap();
        match context.transact(self.message.clone(), 1000) {
            Ok(_) => self.execute_results.borrow_mut().remove(0),
            Err(e) => Err(Transmission(format!("{:?}", e))),
        }
    }
}

impl MockCommand {
    pub fn new(message: MessageBody) -> Self {
        Self {
            message,
            execute_results: RefCell::new(vec![]),
        }
    }

    pub fn execute_result(self, result: Result<(), CommandError>) -> Self {
        self.execute_results.borrow_mut().push(result);
        self
    }
}

#[derive(Clone, Debug)]
pub struct TestWrite {
    write_tx: Sender<String>,
}

impl Write for TestWrite {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        let len = buf.len();
        let string = String::from_utf8(buf.to_vec()).unwrap();
        self.write_tx.send(string).unwrap();
        Ok(len)
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        Ok(())
    }
}

impl TestWrite {
    pub fn new(write_tx: Sender<String>) -> Self {
        Self { write_tx }
    }
}

#[derive(Clone, Debug)]
pub struct TestStreamFactory {
    stdout_opt: RefCell<Option<TestWrite>>,
    stderr_opt: RefCell<Option<TestWrite>>,
}

impl StreamFactory for TestStreamFactory {
    fn make(&self) -> (Box<dyn Write>, Box<dyn Write>) {
        let stdout = self.stdout_opt.borrow_mut().take().unwrap();
        let stderr = self.stderr_opt.borrow_mut().take().unwrap();
        (Box::new(stdout), Box::new(stderr))
    }
}

impl TestStreamFactory {
    pub fn new() -> (TestStreamFactory, TestStreamFactoryHandle) {
        let (stdout_tx, stdout_rx) = unbounded();
        let (stderr_tx, stderr_rx) = unbounded();
        let stdout = TestWrite::new(stdout_tx);
        let stderr = TestWrite::new(stderr_tx);
        let factory = TestStreamFactory {
            stdout_opt: RefCell::new(Some(stdout)),
            stderr_opt: RefCell::new(Some(stderr)),
        };
        let handle = TestStreamFactoryHandle {
            stdout_rx,
            stderr_rx,
        };
        (factory, handle)
    }

    pub fn clone_senders(&self) -> (Sender<String>, Sender<String>) {
        let stdout = self
            .stdout_opt
            .borrow_mut()
            .as_ref()
            .unwrap()
            .write_tx
            .clone();
        let stderr = self
            .stderr_opt
            .borrow_mut()
            .as_ref()
            .unwrap()
            .write_tx
            .clone();
        (stdout, stderr)
    }
}

#[derive(Clone, Debug)]
pub struct TestStreamFactoryHandle {
    stdout_rx: Receiver<String>,
    stderr_rx: Receiver<String>,
}

impl TestStreamFactoryHandle {
    pub fn stdout_so_far(&self) -> String {
        Self::text_so_far(&self.stdout_rx)
    }

    pub fn stderr_so_far(&self) -> String {
        Self::text_so_far(&self.stderr_rx)
    }

    fn text_so_far(rx: &Receiver<String>) -> String {
        let mut accum = String::new();
        let mut retries_left = 5;
        loop {
            match rx.try_recv() {
                Ok(s) => {
                    accum.push_str(&s);
                    retries_left = 5;
                }
                Err(TryRecvError::Empty) => {
                    retries_left -= 1;
                    if retries_left <= 0 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }
        accum
    }
}

#[derive(Clone)]
pub struct MixingStdout {
    channel_half: Sender<String>,
}

impl MixingStdout {
    pub fn new(sender: Sender<String>) -> Self {
        MixingStdout {
            channel_half: sender,
        }
    }
}

impl Write for MixingStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.channel_half
            .send(std::str::from_utf8(buf).unwrap().to_string())
            .unwrap();
        Ok(0)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn write_fmt(&mut self, fmt: Arguments<'_>) -> std::io::Result<()> {
        self.channel_half.send(fmt.to_string()).unwrap();
        Ok(())
    }
}

pub struct InterfaceMock {
    make_result: Arc<Mutex<Vec<Result<TerminalReal, String>>>>,
}

impl TerminalInterfaceFactory for InterfaceMock {
    fn make(&self) -> Result<TerminalReal, String> {
        self.make_result.lock().unwrap().remove(0)
    }
}

impl InterfaceMock {
    pub fn new() -> Self {
        Self {
            make_result: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn make_result(self, result: Result<TerminalReal, String>) -> Self {
        self.make_result.lock().unwrap().push(result);
        self
    }
}

#[derive(Clone)]
pub struct TerminalPassiveMock {
    read_line_result: Arc<Mutex<Vec<TerminalEvent>>>,
}

impl Terminal for TerminalPassiveMock {
    fn read_line(&self) -> TerminalEvent {
        self.read_line_result.lock().unwrap().remove(0)
    }
}

impl TerminalPassiveMock {
    pub fn new() -> Self {
        Self {
            read_line_result: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn read_line_result(self, result: TerminalEvent) -> Self {
        self.read_line_result.lock().unwrap().push(result);
        self
    }
}

//mock incorporating a terminal very similar to the real one, which is run as in-memory though

pub struct TerminalActiveMock {
    in_memory_terminal: Interface<MemoryTerminal>,
    reference: MemoryTerminal,
    user_input: Arc<Mutex<Vec<String>>>,
}

impl Terminal for TerminalActiveMock {
    fn provide_lock(&self) -> Box<dyn WriterGeneric + '_> {
        Box::new(self.in_memory_terminal.lock_writer_append().unwrap())
    }

    fn read_line(&self) -> TerminalEvent {
        let line = self.user_input.lock().unwrap().borrow_mut().remove(0);
        self.reference.write(&format!("{}*/-", line));
        TerminalEvent::CommandLine(line)
    }

    fn add_history_unique(&self, line: String) {
        self.in_memory_terminal.add_history_unique(line)
    }

    #[cfg(test)]
    fn test_interface(&self) -> MemoryTerminal {
        self.reference.clone()
    }
}

impl TerminalActiveMock {
    pub fn new() -> Self {
        let memory_terminal_instance = MemoryTerminal::new();
        Self {
            in_memory_terminal: Interface::with_term(
                "test only terminal",
                memory_terminal_instance.clone(),
            )
            .unwrap(),
            reference: memory_terminal_instance,
            user_input: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn read_line_result(self, line: String) -> Self {
        self.user_input
            .lock()
            .unwrap()
            .borrow_mut()
            .push(format!("{}\n", line));
        self
    }
}

#[derive(Default)]
pub struct InterfaceRawMock {
    read_line_result: Arc<Mutex<Vec<std::io::Result<ReadResult>>>>,
    add_history_unique_params: Arc<Mutex<Vec<String>>>,
}

impl InterfaceRaw for InterfaceRawMock {
    fn read_line(&self) -> std::io::Result<ReadResult> {
        self.read_line_result.lock().unwrap().remove(0)
    }

    fn add_history_unique(&self, line: String) {
        self.add_history_unique_params.lock().unwrap().push(line)
    }

    fn lock_writer_append(&self) -> std::io::Result<Box<dyn WriterGeneric + 'static>> {
        intentionally_blank!()
    }
}

impl InterfaceRawMock {
    pub fn new() -> Self {
        Self {
            read_line_result: Arc::new(Mutex::new(vec![])),
            add_history_unique_params: Arc::new(Mutex::new(vec![])),
        }
    }
    pub fn read_line_result(self, result: std::io::Result<ReadResult>) -> Self {
        self.read_line_result.lock().unwrap().push(result);
        self
    }

    pub fn add_history_unique_params(mut self, params: Arc<Mutex<Vec<String>>>) -> Self {
        self.add_history_unique_params = params;
        self
    }
}
