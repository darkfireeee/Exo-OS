pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VirtqAvailHeader {
    pub flags: u16,
    pub idx: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueError {
    Full,
    Empty,
    DescriptorOutOfRange,
}

pub struct DescriptorFreeList<const N: usize> {
    free: [u16; N],
    len: usize,
}

impl<const N: usize> DescriptorFreeList<N> {
    pub const fn new() -> Self {
        let mut free = [0u16; N];
        let mut i = 0;
        while i < N {
            free[i] = (N - 1 - i) as u16;
            i += 1;
        }
        Self { free, len: N }
    }

    pub const fn available(&self) -> usize {
        self.len
    }

    pub fn alloc(&mut self) -> Result<u16, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Full);
        }
        self.len -= 1;
        Ok(self.free[self.len])
    }

    pub fn free(&mut self, id: u16) -> Result<(), QueueError> {
        if id as usize >= N {
            return Err(QueueError::DescriptorOutOfRange);
        }
        if self.len == N {
            return Err(QueueError::Full);
        }
        self.free[self.len] = id;
        self.len += 1;
        Ok(())
    }
}

impl<const N: usize> Default for DescriptorFreeList<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct SubmissionRing<const N: usize> {
    ring: [u16; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<const N: usize> SubmissionRing<N> {
    pub const fn new() -> Self {
        Self {
            ring: [0; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, desc: u16) -> Result<(), QueueError> {
        if self.len == N {
            return Err(QueueError::Full);
        }
        self.ring[self.tail] = desc;
        self.tail = (self.tail + 1) % N;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Result<u16, QueueError> {
        if self.len == 0 {
            return Err(QueueError::Empty);
        }
        let desc = self.ring[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Ok(desc)
    }
}

impl<const N: usize> Default for SubmissionRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn chain_two(first: &mut VirtqDesc, first_id: u16, second: &mut VirtqDesc, second_id: u16) {
    first.flags |= VIRTQ_DESC_F_NEXT;
    first.next = second_id;
    second.flags &= !VIRTQ_DESC_F_NEXT;
    second.next = first_id;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_list_allocates_and_frees() {
        let mut list = DescriptorFreeList::<4>::new();
        assert_eq!(list.available(), 4);
        let a = list.alloc().unwrap();
        let b = list.alloc().unwrap();
        assert_ne!(a, b);
        assert_eq!(list.available(), 2);
        list.free(a).unwrap();
        assert_eq!(list.available(), 3);
    }

    #[test]
    fn submission_ring_roundtrip() {
        let mut ring = SubmissionRing::<2>::new();
        ring.push(7).unwrap();
        ring.push(9).unwrap();
        assert_eq!(ring.push(11), Err(QueueError::Full));
        assert_eq!(ring.pop(), Ok(7));
        assert_eq!(ring.pop(), Ok(9));
        assert_eq!(ring.pop(), Err(QueueError::Empty));
    }

    #[test]
    fn chains_descriptors() {
        let mut a = VirtqDesc::default();
        let mut b = VirtqDesc::default();
        chain_two(&mut a, 1, &mut b, 2);
        assert_eq!(a.flags & VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_NEXT);
        assert_eq!(a.next, 2);
        assert_eq!(b.next, 1);
    }
}
