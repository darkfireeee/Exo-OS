use exo_syscall_abi as syscall;

const MAX_XDP_PROGRAMS: usize = 16;

#[derive(Clone, Copy)]
struct ProgramRecord {
    active: bool,
    owner_pid: u32,
    prog_id: u32,
    flags: u32,
    packets: u64,
    drops: u64,
}

impl ProgramRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            owner_pid: 0,
            prog_id: 0,
            flags: 0,
            packets: 0,
            drops: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ProgramSnapshot {
    pub owner_pid: u32,
    pub prog_id: u32,
    pub flags: u32,
    pub packets: u64,
    pub drops: u64,
}

pub struct XdpProgramTable {
    programs: [ProgramRecord; MAX_XDP_PROGRAMS],
}

impl XdpProgramTable {
    pub const fn new() -> Self {
        Self {
            programs: [ProgramRecord::empty(); MAX_XDP_PROGRAMS],
        }
    }

    pub fn attach(
        &mut self,
        owner_pid: u32,
        prog_id: u32,
        flags: u32,
    ) -> Result<ProgramSnapshot, i64> {
        if let Some(idx) = self
            .programs
            .iter()
            .position(|entry| entry.active && entry.owner_pid == owner_pid)
        {
            self.programs[idx].prog_id = prog_id;
            self.programs[idx].flags = flags;
            return Ok(self.snapshot(owner_pid).unwrap());
        }

        let Some(idx) = self.programs.iter().position(|entry| !entry.active) else {
            return Err(syscall::ENOSPC);
        };

        self.programs[idx] = ProgramRecord {
            active: true,
            owner_pid,
            prog_id,
            flags,
            packets: 0,
            drops: 0,
        };
        Ok(self.snapshot(owner_pid).unwrap())
    }

    pub fn record_packet(&mut self, owner_pid: u32, dropped: bool) {
        if let Some(idx) = self
            .programs
            .iter()
            .position(|entry| entry.active && entry.owner_pid == owner_pid)
        {
            self.programs[idx].packets = self.programs[idx].packets.saturating_add(1);
            if dropped {
                self.programs[idx].drops = self.programs[idx].drops.saturating_add(1);
            }
        }
    }

    pub fn snapshot(&self, owner_pid: u32) -> Option<ProgramSnapshot> {
        let program = self
            .programs
            .iter()
            .find(|entry| entry.active && entry.owner_pid == owner_pid)?;
        Some(ProgramSnapshot {
            owner_pid: program.owner_pid,
            prog_id: program.prog_id,
            flags: program.flags,
            packets: program.packets,
            drops: program.drops,
        })
    }

    pub fn count(&self) -> u32 {
        self.programs.iter().filter(|entry| entry.active).count() as u32
    }
}
