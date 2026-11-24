use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{Event, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

pub(super) struct Idle {
    queue: EventQueue<Status>,
    notifier: ExtIdleNotifierV1,
    seat: WlSeat,
    notification: Option<ExtIdleNotificationV1>,
    status: Status,
}

#[derive(Default)]
struct Status {
    activity: u32,
    idle: bool,
    timeout: u32,
}

enum Watch {
    Activity,
    Timeout(u32),
}

delegate_noop!(Status: ignore WlSeat);
delegate_noop!(Status: ignore ExtIdleNotifierV1);

impl Dispatch<WlRegistry, GlobalListContents> for Status {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, Watch> for Status {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: Event,
        watch: &Watch,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match (watch, event) {
            (Watch::Activity, Event::Resumed) => state.activity = state.activity.wrapping_add(1),
            (Watch::Timeout(timeout), Event::Idled) if *timeout == state.timeout => {
                state.idle = true
            }
            (Watch::Timeout(timeout), Event::Resumed) if *timeout == state.timeout => {
                state.idle = false
            }
            _ => (),
        }
    }
}

impl Idle {
    pub(super) fn connect() -> Option<Self> {
        let connection = Connection::connect_to_env().ok()?;
        let (globals, queue) = registry_queue_init::<Status>(&connection).ok()?;
        let handle = queue.handle();
        let notifier: ExtIdleNotifierV1 = globals.bind(&handle, 1..=1, ()).ok()?;
        let seat: WlSeat = globals.bind(&handle, 1..=1, ()).ok()?;
        notifier.get_idle_notification(200, &seat, &handle, Watch::Activity);
        queue.flush().ok()?;
        Some(Self {
            queue,
            notifier,
            seat,
            notification: None,
            status: Status::default(),
        })
    }

    pub(super) fn poll(&mut self, timeout: u32) -> Option<(u32, u32)> {
        if self.status.timeout != timeout {
            if let Some(old) = self.notification.take() {
                old.destroy();
            }
            self.status.timeout = timeout;
            self.status.idle = false;
            if timeout > 0 {
                self.notification = Some(self.notifier.get_idle_notification(
                    timeout,
                    &self.seat,
                    &self.queue.handle(),
                    Watch::Timeout(timeout),
                ));
            }
        }
        self.queue.flush().ok()?;
        if let Some(read) = self.queue.prepare_read() {
            let mut fds = [PollFd::from_borrowed_fd(
                read.connection_fd(),
                PollFlags::IN,
            )];
            if poll(&mut fds, Some(&Timespec::default())).is_ok_and(|ready| ready > 0) {
                read.read().ok()?;
            }
        }
        self.queue.dispatch_pending(&mut self.status).ok()?;
        let idle = if self.status.idle { timeout } else { 0 };
        Some((self.status.activity, idle))
    }
}
