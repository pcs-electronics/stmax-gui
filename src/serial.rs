use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use eframe::egui;
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_serial::{
    available_ports, ClearBuffer, SerialPort, SerialPortBuilderExt, SerialPortInfo, SerialPortType,
    SerialStream, UsbPortInfo,
};

use crate::protocol::{
    first_nonempty_line, normalize_response_for_display, response_is_err, DeviceForm,
    DEVICE_BAUD_RATE, QUIET_TIMEOUT_MS, STARTUP_DELAY_MS,
};

const MAX_LOG_LINES: usize = 300;

#[derive(Clone, Debug, Default)]
pub struct PortSummary {
    pub port_name: String,
    pub summary: String,
    pub is_preferred_device: bool,
}

#[derive(Clone, Debug)]
pub struct SerialSnapshot {
    pub connection_status: String,
    pub ports: Vec<PortSummary>,
    pub connected_port: Option<String>,
    pub connected_usb_summary: Option<String>,
    pub tx_commands: u64,
    pub rx_lines: u64,
    pub last_event: String,
    pub last_error: Option<String>,
    pub log_lines: Vec<String>,
    pub port_scan_generation: u64,
    pub readback_generation: u64,
    pub readback_form: Option<DeviceForm>,
    pub last_response_text: Option<String>,
    pub busy: bool,
}

impl Default for SerialSnapshot {
    fn default() -> Self {
        Self {
            connection_status: "idle".to_owned(),
            ports: Vec::new(),
            connected_port: None,
            connected_usb_summary: None,
            tx_commands: 0,
            rx_lines: 0,
            last_event: "serial controller starting".to_owned(),
            last_error: None,
            log_lines: vec!["[system] serial controller started".to_owned()],
            port_scan_generation: 0,
            readback_generation: 0,
            readback_form: None,
            last_response_text: None,
            busy: false,
        }
    }
}

pub struct SerialController {
    command_tx: mpsc::UnboundedSender<ControllerCommand>,
    snapshot_rx: watch::Receiver<SerialSnapshot>,
}

impl SerialController {
    pub fn spawn(runtime: &Runtime, repaint_ctx: egui::Context) -> Self {
        let initial = SerialSnapshot::default();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (snapshot_tx, snapshot_rx) = watch::channel(initial.clone());

        runtime.spawn(controller_task(command_rx, snapshot_tx, repaint_ctx));

        Self {
            command_tx,
            snapshot_rx,
        }
    }

    pub fn snapshot(&self) -> SerialSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    pub fn try_snapshot(&mut self) -> Option<SerialSnapshot> {
        match self.snapshot_rx.has_changed() {
            Ok(true) => Some(self.snapshot_rx.borrow_and_update().clone()),
            Ok(false) => None,
            Err(_) => {
                let mut snapshot = self.snapshot_rx.borrow().clone();
                snapshot.connection_status = "controller stopped".to_owned();
                snapshot.last_error = Some("serial state channel closed".to_owned());
                Some(snapshot)
            }
        }
    }

    pub fn refresh_ports(&self) {
        let _ = self.command_tx.send(ControllerCommand::RefreshPorts);
    }

    pub fn connect(&self, port_name: String) {
        let _ = self.command_tx.send(ControllerCommand::Connect {
            port_name,
            baud_rate: DEVICE_BAUD_RATE,
        });
    }

    pub fn disconnect(&self) {
        let _ = self.command_tx.send(ControllerCommand::Disconnect);
    }

    pub fn read_config(&self) {
        let _ = self.command_tx.send(ControllerCommand::ReadConfig);
    }

    pub fn save_config(&self, form: DeviceForm) {
        let _ = self.command_tx.send(ControllerCommand::SaveConfig(form));
    }

    pub fn factory_defaults(&self) {
        let _ = self.command_tx.send(ControllerCommand::FactoryDefaults);
    }
}

#[derive(Debug)]
enum ControllerCommand {
    RefreshPorts,
    Connect { port_name: String, baud_rate: u32 },
    Disconnect,
    ReadConfig,
    SaveConfig(DeviceForm),
    FactoryDefaults,
}

#[derive(Debug)]
enum ReaderEvent {
    Line(String),
    Error(String),
}

async fn controller_task(
    mut command_rx: mpsc::UnboundedReceiver<ControllerCommand>,
    snapshot_tx: watch::Sender<SerialSnapshot>,
    repaint_ctx: egui::Context,
) {
    let mut snapshot = SerialSnapshot::default();
    let (reader_event_tx, mut reader_event_rx) = mpsc::unbounded_channel();
    let mut writer: Option<WriteHalf<SerialStream>> = None;
    let mut reader_task: Option<JoinHandle<()>> = None;

    refresh_ports(&mut snapshot);
    publish(&snapshot_tx, &repaint_ctx, &snapshot);

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                match command {
                    ControllerCommand::RefreshPorts => {
                        refresh_ports(&mut snapshot);
                    }
                    ControllerCommand::Connect { port_name, baud_rate } => {
                        disconnect_active_connection(&mut writer, &mut reader_task, &mut snapshot, false);
                        connect_port(
                            &port_name,
                            baud_rate,
                            &reader_event_tx,
                            &mut writer,
                            &mut reader_task,
                            &mut snapshot,
                        ).await;
                    }
                    ControllerCommand::Disconnect => {
                        disconnect_active_connection(&mut writer, &mut reader_task, &mut snapshot, true);
                    }
                    ControllerCommand::ReadConfig => {
                        begin_operation(&mut snapshot, "reading", "reading transmitter settings");
                        publish(&snapshot_tx, &repaint_ctx, &snapshot);

                        let result = perform_readback(
                            &mut writer,
                            &mut reader_task,
                            &mut reader_event_rx,
                            &snapshot_tx,
                            &repaint_ctx,
                            &mut snapshot,
                        )
                        .await;

                        finish_operation(&mut snapshot, result);
                    }
                    ControllerCommand::SaveConfig(form) => {
                        begin_operation(
                            &mut snapshot,
                            "saving",
                            "sending configuration to transmitter",
                        );
                        publish(&snapshot_tx, &repaint_ctx, &snapshot);

                        let result = perform_save(
                            form,
                            &mut writer,
                            &mut reader_task,
                            &mut reader_event_rx,
                            &snapshot_tx,
                            &repaint_ctx,
                            &mut snapshot,
                        )
                        .await;

                        finish_operation(&mut snapshot, result);
                    }
                    ControllerCommand::FactoryDefaults => {
                        begin_operation(
                            &mut snapshot,
                            "resetting",
                            "restoring factory defaults",
                        );
                        publish(&snapshot_tx, &repaint_ctx, &snapshot);

                        let result = perform_factory_defaults(
                            &mut writer,
                            &mut reader_task,
                            &mut reader_event_rx,
                            &snapshot_tx,
                            &repaint_ctx,
                            &mut snapshot,
                        )
                        .await;

                        finish_operation(&mut snapshot, result);
                    }
                }

                publish(&snapshot_tx, &repaint_ctx, &snapshot);
            }
            Some(event) = reader_event_rx.recv() => {
                handle_background_event(
                    event,
                    &mut writer,
                    &mut reader_task,
                    &mut snapshot,
                );
                publish(&snapshot_tx, &repaint_ctx, &snapshot);
            }
            else => break,
        }
    }
}

fn begin_operation(snapshot: &mut SerialSnapshot, active_status: &str, active_event: &str) {
    snapshot.busy = true;
    snapshot.last_error = None;
    snapshot.last_event = active_event.to_owned();
    if snapshot.connected_port.is_some() {
        snapshot.connection_status = active_status.to_owned();
    }
}

fn finish_operation(snapshot: &mut SerialSnapshot, result: Result<&'static str, String>) {
    snapshot.busy = false;

    match result {
        Ok(success_text) => {
            snapshot.last_error = None;
            snapshot.last_event = success_text.to_owned();
            if snapshot.connected_port.is_some() {
                snapshot.connection_status = "connected".to_owned();
            }
        }
        Err(error) => {
            if snapshot.connected_port.is_some() && !snapshot.connection_status.ends_with("error") {
                snapshot.connection_status = "connected".to_owned();
            }
            snapshot.last_error = Some(error.clone());
            snapshot.last_event = "last operation failed".to_owned();
            push_log(snapshot, format!("[error] {error}"));
        }
    }
}

async fn perform_readback(
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    reader_event_rx: &mut mpsc::UnboundedReceiver<ReaderEvent>,
    snapshot_tx: &watch::Sender<SerialSnapshot>,
    repaint_ctx: &egui::Context,
    snapshot: &mut SerialSnapshot,
) -> Result<&'static str, String> {
    let response = run_device_command(
        "?",
        writer,
        reader_task,
        reader_event_rx,
        snapshot_tx,
        repaint_ctx,
        snapshot,
    )
    .await?;

    let form = DeviceForm::from_info_response(&response)?;
    snapshot.readback_generation += 1;
    snapshot.readback_form = Some(form);
    snapshot.last_response_text = Some(normalize_response_for_display(&response));

    Ok("read settings from transmitter")
}

async fn perform_save(
    form: DeviceForm,
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    reader_event_rx: &mut mpsc::UnboundedReceiver<ReaderEvent>,
    snapshot_tx: &watch::Sender<SerialSnapshot>,
    repaint_ctx: &egui::Context,
    snapshot: &mut SerialSnapshot,
) -> Result<&'static str, String> {
    let commands = form.build_save_commands()?;

    for command in commands {
        let response = run_device_command(
            &command,
            writer,
            reader_task,
            reader_event_rx,
            snapshot_tx,
            repaint_ctx,
            snapshot,
        )
        .await?;

        snapshot.last_response_text = Some(normalize_response_for_display(&response));
    }

    perform_readback(
        writer,
        reader_task,
        reader_event_rx,
        snapshot_tx,
        repaint_ctx,
        snapshot,
    )
    .await?;

    Ok("saved settings to transmitter")
}

async fn perform_factory_defaults(
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    reader_event_rx: &mut mpsc::UnboundedReceiver<ReaderEvent>,
    snapshot_tx: &watch::Sender<SerialSnapshot>,
    repaint_ctx: &egui::Context,
    snapshot: &mut SerialSnapshot,
) -> Result<&'static str, String> {
    let response = run_device_command(
        "config-defaults",
        writer,
        reader_task,
        reader_event_rx,
        snapshot_tx,
        repaint_ctx,
        snapshot,
    )
    .await?;
    snapshot.last_response_text = Some(normalize_response_for_display(&response));

    perform_readback(
        writer,
        reader_task,
        reader_event_rx,
        snapshot_tx,
        repaint_ctx,
        snapshot,
    )
    .await?;

    Ok("restored factory defaults on transmitter")
}

async fn run_device_command(
    command: &str,
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    reader_event_rx: &mut mpsc::UnboundedReceiver<ReaderEvent>,
    snapshot_tx: &watch::Sender<SerialSnapshot>,
    repaint_ctx: &egui::Context,
    snapshot: &mut SerialSnapshot,
) -> Result<String, String> {
    if writer.is_none() {
        return Err("no serial port connected".to_owned());
    }

    let mut payload = command.as_bytes().to_vec();
    payload.extend_from_slice(b"\r\n");

    let write_result = {
        let port = writer.as_mut().expect("writer presence checked above");
        match port.write_all(&payload).await {
            Ok(()) => port.flush().await,
            Err(error) => Err(error),
        }
    };

    match write_result {
        Ok(()) => {
            snapshot.tx_commands += 1;
            snapshot.last_error = None;
            snapshot.last_event = format!("sent command {}", snapshot.tx_commands);
            push_log(snapshot, format!("> {command}"));
            publish(snapshot_tx, repaint_ctx, snapshot);
        }
        Err(error) => {
            handle_writer_error(error, writer, reader_task, snapshot);
            return Err("serial write failed".to_owned());
        }
    }

    let mut lines = Vec::new();

    loop {
        match tokio::time::timeout(
            Duration::from_millis(QUIET_TIMEOUT_MS),
            reader_event_rx.recv(),
        )
        .await
        {
            Ok(Some(ReaderEvent::Line(line))) => {
                snapshot.rx_lines += 1;
                snapshot.last_error = None;
                snapshot.last_event = format!("received line {}", snapshot.rx_lines);
                push_log(snapshot, format!("< {line}"));
                lines.push(line);
                publish(snapshot_tx, repaint_ctx, snapshot);
            }
            Ok(Some(ReaderEvent::Error(error))) => {
                handle_reader_error(error.clone(), writer, reader_task, snapshot);
                publish(snapshot_tx, repaint_ctx, snapshot);
                return Err(error);
            }
            Ok(None) => {
                return Err("serial reader task stopped".to_owned());
            }
            Err(_) if lines.is_empty() => {
                return Err(format!("no response received after sending `{command}`"));
            }
            Err(_) => {
                break;
            }
        }
    }

    let response = lines.join("\n");
    if response_is_err(&response) {
        let detail = first_nonempty_line(&response).unwrap_or("ERR");
        return Err(format!("device rejected `{command}` with `{detail}`"));
    }

    Ok(response)
}

fn handle_background_event(
    event: ReaderEvent,
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    snapshot: &mut SerialSnapshot,
) {
    match event {
        ReaderEvent::Line(line) => {
            snapshot.rx_lines += 1;
            snapshot.last_error = None;
            snapshot.last_event = "received unsolicited device output".to_owned();
            push_log(snapshot, format!("< {line}"));
        }
        ReaderEvent::Error(error) => {
            handle_reader_error(error, writer, reader_task, snapshot);
        }
    }
}

fn refresh_ports(snapshot: &mut SerialSnapshot) {
    match available_ports() {
        Ok(ports) => {
            snapshot.ports = ports.into_iter().map(port_summary).collect();
            snapshot.port_scan_generation += 1;
            snapshot.last_error = None;
            snapshot.last_event = format!("discovered {} serial port(s)", snapshot.ports.len());
            if snapshot.connected_port.is_none() {
                snapshot.connection_status = "ports refreshed".to_owned();
            }
        }
        Err(error) => {
            snapshot.last_error = Some(format!("failed to enumerate serial ports: {error}"));
            snapshot.last_event = "serial port enumeration failed".to_owned();
            snapshot.connection_status = "enumeration error".to_owned();
            push_log(
                snapshot,
                format!("[error] failed to enumerate serial ports: {error}"),
            );
        }
    }
}

async fn connect_port(
    port_name: &str,
    baud_rate: u32,
    reader_event_tx: &mpsc::UnboundedSender<ReaderEvent>,
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    snapshot: &mut SerialSnapshot,
) {
    let builder = tokio_serial::new(port_name, baud_rate).dtr_on_open(true);

    match builder.open_native_async() {
        Ok(stream) => {
            let usb_summary = snapshot
                .ports
                .iter()
                .find(|port| port.port_name == port_name)
                .map(|port| port.summary.clone())
                .filter(|summary| !summary.is_empty());

            #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
            let mut stream = stream;

            #[cfg(target_os = "windows")]
            {
                if let Err(error) = stream.write_data_terminal_ready(true) {
                    push_log(
                        snapshot,
                        format!("[warning] failed to assert DTR on {port_name}: {error}"),
                    );
                }

                if let Err(error) = stream.write_request_to_send(true) {
                    push_log(
                        snapshot,
                        format!("[warning] failed to assert RTS on {port_name}: {error}"),
                    );
                }
            }

            tokio::time::sleep(Duration::from_millis(STARTUP_DELAY_MS)).await;

            if let Err(error) = stream.clear(ClearBuffer::All) {
                push_log(
                    snapshot,
                    format!("[warning] failed to clear serial buffers on {port_name}: {error}"),
                );
            }

            let (reader, write_half) = tokio::io::split(stream);
            let event_tx = reader_event_tx.clone();
            let port_name_owned = port_name.to_owned();

            *writer = Some(write_half);
            *reader_task = Some(tokio::spawn(async move {
                read_lines_task(port_name_owned, reader, event_tx).await;
            }));

            snapshot.connection_status = "connected".to_owned();
            snapshot.connected_port = Some(port_name.to_owned());
            snapshot.connected_usb_summary = usb_summary;
            snapshot.last_error = None;
            snapshot.last_event = format!("connected to {port_name} at {baud_rate} baud");
            push_log(
                snapshot,
                format!("[system] connected to {port_name} at {baud_rate} baud"),
            );
        }
        Err(error) => {
            snapshot.connection_status = "connect error".to_owned();
            snapshot.connected_port = None;
            snapshot.connected_usb_summary = None;
            snapshot.last_error = Some(format!("failed to open {port_name}: {error}"));
            snapshot.last_event = format!("failed to connect to {port_name}");
            push_log(
                snapshot,
                format!("[error] failed to open {port_name}: {error}"),
            );
        }
    }
}

fn handle_writer_error(
    error: io::Error,
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    snapshot: &mut SerialSnapshot,
) {
    *writer = None;
    if let Some(task) = reader_task.take() {
        task.abort();
    }

    snapshot.connection_status = "write error".to_owned();
    snapshot.connected_port = None;
    snapshot.connected_usb_summary = None;
    snapshot.last_error = Some(format!("serial write failed: {error}"));
    snapshot.last_event = "disconnected after write failure".to_owned();
    push_log(snapshot, format!("[error] serial write failed: {error}"));
}

fn handle_reader_error(
    error: String,
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    snapshot: &mut SerialSnapshot,
) {
    *writer = None;
    if let Some(task) = reader_task.take() {
        task.abort();
    }

    snapshot.connection_status = "read error".to_owned();
    snapshot.connected_port = None;
    snapshot.connected_usb_summary = None;
    snapshot.last_error = Some(error.clone());
    snapshot.last_event = "reader task failed".to_owned();
    push_log(snapshot, format!("[error] {error}"));
}

fn disconnect_active_connection(
    writer: &mut Option<WriteHalf<SerialStream>>,
    reader_task: &mut Option<JoinHandle<()>>,
    snapshot: &mut SerialSnapshot,
    explicit: bool,
) {
    *writer = None;
    if let Some(task) = reader_task.take() {
        task.abort();
    }

    if explicit || snapshot.connected_port.is_some() {
        snapshot.connection_status = "disconnected".to_owned();
        snapshot.connected_port = None;
        snapshot.connected_usb_summary = None;
        snapshot.last_event = if explicit {
            "serial port disconnected by user".to_owned()
        } else {
            "serial port disconnected".to_owned()
        };
        snapshot.last_error = None;
        push_log(snapshot, format!("[system] {}", snapshot.last_event));
    }
}

async fn read_lines_task(
    port_name: String,
    mut reader: tokio::io::ReadHalf<SerialStream>,
    event_tx: mpsc::UnboundedSender<ReaderEvent>,
) {
    let mut read_buffer = [0_u8; 512];
    let mut line_buffer = Vec::with_capacity(256);

    loop {
        match reader.read(&mut read_buffer).await {
            Ok(0) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            Ok(bytes_read) => {
                for &byte in &read_buffer[..bytes_read] {
                    if matches!(byte, b'\r' | b'\n') {
                        if !line_buffer.is_empty() {
                            let line = String::from_utf8_lossy(&line_buffer).into_owned();
                            let _ = event_tx.send(ReaderEvent::Line(line));
                            line_buffer.clear();
                        }
                    } else {
                        line_buffer.push(byte);
                    }
                }
            }
            Err(error) => {
                #[cfg(target_os = "windows")]
                if error.raw_os_error() == Some(995) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }

                let _ = event_tx.send(ReaderEvent::Error(format!(
                    "serial read failed on {port_name}: {error}"
                )));
                break;
            }
        }
    }
}

fn publish(
    snapshot_tx: &watch::Sender<SerialSnapshot>,
    repaint_ctx: &egui::Context,
    snapshot: &SerialSnapshot,
) {
    let _ = snapshot_tx.send(snapshot.clone());
    repaint_ctx.request_repaint();
}

fn port_summary(port: SerialPortInfo) -> PortSummary {
    let is_preferred_device = is_preferred_device(&port);
    let summary = match &port.port_type {
        SerialPortType::UsbPort(info) => format_usb_summary(info),
        SerialPortType::PciPort => "pci".to_owned(),
        SerialPortType::BluetoothPort => "bluetooth".to_owned(),
        SerialPortType::Unknown => "unknown".to_owned(),
    };

    PortSummary {
        port_name: port.port_name,
        summary,
        is_preferred_device,
    }
}

fn is_preferred_device(port: &SerialPortInfo) -> bool {
    if matches!(port.port_name.as_str(), "/dev/ttyUSB0" | "/dev/ttyACM0") {
        return true;
    }

    match &port.port_type {
        SerialPortType::UsbPort(info) => {
            let mut haystack = String::new();
            if let Some(value) = &info.product {
                haystack.push_str(value);
                haystack.push(' ');
            }
            if let Some(value) = &info.manufacturer {
                haystack.push_str(value);
                haystack.push(' ');
            }
            if let Some(value) = &info.serial_number {
                haystack.push_str(value);
            }
            let haystack = haystack.to_ascii_lowercase();

            [
                "usb serial",
                "wch",
                "ch340",
                "cp210",
                "ftdi",
                "arduino",
                "serial",
            ]
            .iter()
            .any(|needle| haystack.contains(needle))
        }
        _ => false,
    }
}

fn format_usb_summary(info: &UsbPortInfo) -> String {
    let mut parts = Vec::new();

    if let Some(product) = &info.product {
        parts.push(product.clone());
    }

    if let Some(manufacturer) = &info.manufacturer {
        parts.push(manufacturer.clone());
    }

    parts.push(format!("VID:{:04x} PID:{:04x}", info.vid, info.pid));

    if let Some(serial_number) = &info.serial_number {
        parts.push(format!("SN:{serial_number}"));
    }

    parts.join(" | ")
}

fn push_log(snapshot: &mut SerialSnapshot, line: String) {
    let mut log = VecDeque::from(std::mem::take(&mut snapshot.log_lines));
    log.push_back(line);
    while log.len() > MAX_LOG_LINES {
        log.pop_front();
    }
    snapshot.log_lines = log.into();
}
