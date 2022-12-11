//! Implementation of [`MapArea`] and [`MemorySet`].

use super::{
    frame_alloc, get_num_empty_frame, vpn_range_is_unused, vpn_range_is_used, FrameTracker,
};
use super::{PTEFlags, PageTable, PageTableEntry};
use super::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::{StepByOne, VPNRange};
use crate::config::{MEMORY_END, PAGE_SIZE, TRAMPOLINE, TRAP_CONTEXT, USER_STACK_SIZE};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::*;
use riscv::register::satp;
use spin::Mutex;

extern "C" {
    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();
    fn sbss_with_stack();
    fn ebss();
    fn ekernel();
    fn strampoline();
}

lazy_static! {
    /// a memory set instance through lazy_static! managing kernel space
    pub static ref KERNEL_SPACE: Arc<Mutex<MemorySet>> =
        Arc::new(Mutex::new(MemorySet::new_kernel()));
}

/// memory set structure, controls virtual-memory space
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}

impl MemorySet {
    /// 返回一个初始化后的MemorySet,其中包含一个仅有根节点的PageTable
    /// 创建MemorySet的过程中没有分配用于存储普通数据的物理页，只分配了存储PageTable的物理页
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(), //此处为PageTable分配了一个物理frame，用于存储根页表
            areas: Vec::new(),
        }
    }
    /// 获得self中的页表对应的satp字段（对应一个CSR寄存器）的值
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// Assume that no conflicts.
    /// 将self.vpn_range中的所有vpn都分配一个对应的物理内存中的frame，并为他们在页表中创建页表项；
    /// 无需存入实际
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) {
        self.push(
            MapArea::new(start_va, end_va, MapType::Framed, permission),
            None,
        );
    }
    /// 将self.vpn_range中的所有vpn都分配一个对应的物理内存中的frame，并为他们在页表中创建页表项；
    /// 并将data中的数据都推入分配的物理内存中
    fn push(&mut self, mut map_area: MapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&mut self.page_table, data);
        }
        self.areas.push(map_area);
    }
    /// Mention that trampoline is not collected by areas.
    /// 将跳板放入PageTable(self)中，建立与PhysAddr:strampoline的页表项，
    /// strampoline是在将OS载入内存时“.text.trampoline”这部分数据的起始
    fn map_trampoline(&mut self) {
        self.page_table.map(
            VirtAddr::from(TRAMPOLINE).into(),
            PhysAddr::from(strampoline as usize).into(),
            PTEFlags::R | PTEFlags::X,
        );
    }
    /// Without kernel stacks.
    /// 将OS 对应的“.text, .rodata, .data, .bss”纳入内存管理，
    /// 且由于这几个部分在一开始就被载入内存了，所以vpn和ppn应该是一样的，所以maptype是identitial
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map kernel sections
        info!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        info!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        info!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        info!(
            ".bss [{:#x}, {:#x})",
            sbss_with_stack as usize, ebss as usize
        );
        info!("mapping .text section");
        memory_set.push(
            MapArea::new(
                (stext as usize).into(),
                (etext as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
            ),
            None,
        );
        info!("mapping .rodata section");
        memory_set.push(
            MapArea::new(
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapType::Identical,
                MapPermission::R,
            ),
            None,
        );
        info!("mapping .data section");
        memory_set.push(
            MapArea::new(
                (sdata as usize).into(),
                (edata as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        info!("mapping .bss section");
        memory_set.push(
            MapArea::new(
                (sbss_with_stack as usize).into(),
                (ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        info!("mapping physical memory");
        memory_set.push(
            MapArea::new(
                (ekernel as usize).into(),
                MEMORY_END.into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        memory_set
    }
    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp and entry point.
    /// 为单个app创建页表，同时将app的各个逻辑段（.text, .rodata, .data, .bss）放入新的物理内存中，并为这个app创建user stack (4.6)
    pub fn from_elf(elf_data: &[u8]) -> (Self, usize, usize) {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map program headers of elf, with U flag
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap(); // 用crate xmas_elf 来解析传入的应用 ELF 数据并可以轻松取出各个部分 （4.6）
        let elf_header = elf.header;

        // 取出魔数来判断这个ELF文件是否合法
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");

        let ph_count = elf_header.pt2.ph_count(); // pt2中存储了elf文件的第19行到29行的内容；ph_count==ProgramHeaderCount
        let mut max_end_vpn = VirtPageNum(0);
        // 在for循环中将所有类型为“LOAD”的programhead放入物理内存的应用部分，并这部分物理空间构建的页表项
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                max_end_vpn = map_area.vpn_range.get_end();
                memory_set.push(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                );
            }
        }
        // map user stack with U flags
        let max_end_va: VirtAddr = max_end_vpn.into();
        let mut user_stack_bottom: usize = max_end_va.into();
        // guard page
        user_stack_bottom += PAGE_SIZE;
        let user_stack_top = user_stack_bottom + USER_STACK_SIZE;
        memory_set.push(
            MapArea::new(
                user_stack_bottom.into(),
                user_stack_top.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
            None,
        );

        // map TrapContext
        // 此处未作任何初始化
        memory_set.push(
            MapArea::new(
                TRAP_CONTEXT.into(),
                TRAMPOLINE.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        (
            memory_set,
            user_stack_top,
            elf.header.pt2.entry_point() as usize,
        )
    }

    /// 将OS的自己的页表放入satp这个寄存器中，同时将这个寄存器中的mode字段置为8以启动SV39分页机制。
    /// 与此同时，使用“sfence.vma ”汇编指令清空TLB （4.7）
    pub fn activate(&self) {
        let satp = self.page_table.token();
        unsafe {
            satp::write(satp);
            core::arch::asm!("sfence.vma");
        }
    }
    /// 寻早self中对应于vpn的页表项，如果能够找到，就将页表项拷贝一份并返回
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }

    pub fn mmap(&mut self, start: usize, len: usize, port: usize) -> isize {
        let len_n = (len - 1 + PAGE_SIZE) / PAGE_SIZE;
        let start_n = start / PAGE_SIZE;
        let pt = &mut self.page_table;

        if VirtAddr(start).page_offset() != 0
            || (port & !0x7) != 0
            || port & 0x7 == 0
            || get_num_empty_frame() < len_n
            || !vpn_range_is_unused(pt, start_n, len_n)
        {
            -1
        } else {
            if len == 0 {
                0
            } else {
                let mut map_perm = MapPermission::U;
                if port & 0x1 != 0 {
                    map_perm |= MapPermission::R;
                }
                if port & 0x2 != 0 {
                    map_perm |= MapPermission::W;
                }
                if port & 0x4 != 0 {
                    map_perm |= MapPermission::X;
                }

                self.insert_framed_area(
                    VirtAddr::from(VirtPageNum::from(start_n)),
                    VirtAddr::from(VirtPageNum::from(len_n + start_n)),
                    map_perm,
                );
                0
            }
        }
    }

    pub fn munmap(&mut self, start: usize, len: usize) -> isize {
        let pt = &mut self.page_table;
        if start % PAGE_SIZE != 0 || !vpn_range_is_used(pt, start, len) || len % PAGE_SIZE != 0 {
            -1
        } else {
            let start_va = start / PAGE_SIZE;
            let end_va = (len + start) / PAGE_SIZE;

            // 循环体
            for map_area in &mut self.areas {
                if map_area.vpn_range.get_start().0 == start_va
                    && map_area.vpn_range.get_end().0 <= end_va
                {
                    map_area.unmap(pt);
                    break;
                }
            }
            0
        }
    }
}

/// map area structure, controls a contiguous piece of virtual memory
/// start_va: 虚拟内存的起始地址（4.6）
/// end_va: 虚拟内存的结束地址（4.6）
/// map_tpye: 描述该逻辑段内的所有虚拟页面映射到物理页帧的同一种方式 （identitial/frame两种）（4.6）
/// map_perm: 控制该逻辑段的访问方式，它是页表项标志位 PTEFlags 的一个子集（4.6）
pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
}

impl MapArea {
    /// 返回一个MapArea（逻辑段）, 它描述一段连续地址的虚拟内存 （4.6）
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
    ) -> Self {
        let start_vpn: VirtPageNum = start_va.floor();
        let end_vpn: VirtPageNum = end_va.ceil();
        Self {
            vpn_range: VPNRange::new(start_vpn, end_vpn),
            data_frames: BTreeMap::new(),
            map_type,
            map_perm,
        }
    }

    /// 将单个vpn与物理内空间中的一个frame建立关联，并将相应的页表项放入页表中。
    /// 关于如何为vnp挑选合适的frame： 如果MapType为identital,则vpn和ppn值一样，如果为framed则由frame分配器生成。
    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        match self.map_type {
            MapType::Identical => {
                ppn = PhysPageNum(vpn.0);
            }
            MapType::Framed => {
                let frame = frame_alloc().unwrap();
                ppn = frame.ppn;
                self.data_frames.insert(vpn, frame);
            }
        }
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }
    /// 将vpn在page_table对应的页表项删除，并将对应的物理页回收
    #[allow(unused)]
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        #[allow(clippy::single_match)]
        match self.map_type {
            MapType::Framed => {
                self.data_frames.remove(&vpn);
            }
            _ => {}
        }
        page_table.unmap(vpn);
    }

    /// 将self.vpn_range中的所有vpn都分配一个对应的frame，并为他们在页表中创建页表项
    pub fn map(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.map_one(page_table, vpn);
        }
    }
    /// 将self.vpn_range中的所有vpn对应的页表项都删除，并将相应的物理页回收
    #[allow(unused)]
    pub fn unmap(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }
    /// data: start-aligned but maybe with shorter length
    /// assume that all frames were cleared before
    /// 将切片 data 中的数据拷贝到当前逻辑段实际被内核放置在的各物理页帧上 （4.6）
    pub fn copy_data(&mut self, page_table: &mut PageTable, data: &[u8]) {
        assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len {
                break;
            }
            current_vpn.step();
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
/// map type for memory set: identical or framed
pub enum MapType {
    Identical,
    Framed,
}

bitflags! {
    /// map permission corresponding to that in pte: `R W X U`.
    /// R(Read)/W(Write)/X(eXecute)：分别控制索引到这个页表项的对应虚拟页面是否允许读/写/执行,
    /// U(User)：控制索引到这个页表项的对应虚拟页面是否在 CPU 处于 U 特权级的情况下是否被允许访问
    pub struct MapPermission: u8 {
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
    }
}

#[allow(unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_SPACE.lock();
    let mid_text: VirtAddr = ((stext as usize + etext as usize) / 2).into();
    let mid_rodata: VirtAddr = ((srodata as usize + erodata as usize) / 2).into();
    let mid_data: VirtAddr = ((sdata as usize + edata as usize) / 2).into();
    assert!(!kernel_space
        .page_table
        .translate(mid_text.floor())
        .unwrap()
        .writable());
    assert!(!kernel_space
        .page_table
        .translate(mid_rodata.floor())
        .unwrap()
        .writable());
    assert!(!kernel_space
        .page_table
        .translate(mid_data.floor())
        .unwrap()
        .executable());
    info!("remap_test passed!");
}