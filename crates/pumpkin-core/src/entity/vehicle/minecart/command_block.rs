use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::Entity;

/// Vanilla `MinecartCommandBlock` delegates to `BaseCommandBlock` for its
/// command state; the same subset of fields is kept here without a block
/// position so the entity can move around.
pub struct CommandBlockMinecart {
    pub command: StdMutex<String>,
    pub last_output: StdMutex<String>,
    pub track_output: AtomicBool,
    pub success_count: AtomicU32,
    /// Server tick of the last activation, mirroring vanilla `lastActivated`.
    pub last_activated: AtomicI32,
    /// Snapshot of the entity position/rotation used to build the command
    /// source. Kept in sync right before execution.
    pub position: AtomicCell<Vector3<f64>>,
    pub rotation: AtomicCell<Vector3<f32>>,
}

impl CommandBlockMinecart {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            command: StdMutex::new(String::new()),
            last_output: StdMutex::new(String::new()),
            track_output: AtomicBool::new(true),
            success_count: AtomicU32::new(0),
            last_activated: AtomicI32::new(i32::MIN),
            position: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            rotation: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
        }
    }

    /// Mirrors vanilla `MinecartCommandBlock.activateMinecart`: while an
    /// activator rail powers the cart, the command runs at most once every
    /// 4 ticks.
    pub fn activate(self: &Arc<Self>, entity: &Entity) {
        let Some(server) = entity.world.load().server.upgrade() else {
            return;
        };
        let current_tick = server.tick_count.load(Ordering::Relaxed);
        let last = self.last_activated.load(Ordering::Relaxed);
        // `i32::MIN` is the "never activated" sentinel.
        if last != i32::MIN && current_tick.wrapping_sub(last) < 4 {
            return;
        }
        self.last_activated.store(current_tick, Ordering::Relaxed);
        self.perform_command(entity);
    }

    /// Ports the relevant part of `BaseCommandBlock.performCommand`: reset the
    /// success count and dispatch the stored command with a GM-level command
    /// block source at the minecart's position.
    pub fn perform_command(self: &Arc<Self>, entity: &Entity) {
        self.success_count.store(0, Ordering::Release);

        let command = self
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if command.is_empty() {
            return;
        }

        // `Vanilla` "Searge" easter egg.
        if command.eq_ignore_ascii_case("Searge") {
            *self
                .last_output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = "#itzlipofutzli".to_string();
            self.success_count.store(1, Ordering::Release);
            return;
        }

        self.position.store(entity.pos.load());
        self.rotation.store(entity.rotation());

        let world = entity.world.load_full();
        if !world.level_info.load().game_rules.command_blocks_work {
            return;
        }

        let Some(server) = world.server.upgrade() else {
            return;
        };

        let source = crate::command::CommandSender::CommandBlockMinecart(self.clone(), world)
            .into_source(&server);
        server
            .command_dispatcher
            .load()
            .handle_command(&source, &command);
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string(
            "Command",
            self.command
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        );
        nbt.put_int(
            "SuccessCount",
            self.success_count.load(Ordering::Relaxed).cast_signed(),
        );
        nbt.put_bool("TrackOutput", self.track_output.load(Ordering::Relaxed));
        if self.track_output.load(Ordering::Relaxed) {
            nbt.put_string(
                "LastOutput",
                self.last_output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            );
        }
    }

    pub fn read_nbt(&self, nbt: &NbtCompound) {
        *self
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            nbt.get_string("Command").unwrap_or("").to_string();
        self.success_count.store(
            nbt.get_int("SuccessCount").unwrap_or(0).cast_unsigned(),
            Ordering::Relaxed,
        );
        let track_output = nbt.get_bool("TrackOutput").unwrap_or(true);
        self.track_output.store(track_output, Ordering::Relaxed);
        if track_output {
            *self
                .last_output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                nbt.get_string("LastOutput").unwrap_or("").to_string();
        }
    }
}

impl Default for CommandBlockMinecart {
    fn default() -> Self {
        Self::new()
    }
}
