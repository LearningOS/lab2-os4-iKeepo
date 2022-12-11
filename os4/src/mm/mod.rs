//! Memory management implementation
//! 
//! SV39 page-based virtual-memory architecture for RV64 systems, and
//! everything about memory management, like frame allocator, page table,
//! map area and memory set, is implemented here.
//! 
//! Every task or process has a memory_set to control its virtual memory.


mod address;
mod frame_allocator;
mod heap_allocator;
mod memory_set;
mod page_table;

pub use address::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use address::{StepByOne, VPNRange};
pub use frame_allocator::{frame_alloc, FrameTracker, get_num_empty_frame};
pub use memory_set::remap_test;
pub use memory_set::{MapPermission, MemorySet, KERNEL_SPACE};
pub use page_table::{translated_byte_buffer, vpn_range_is_unused, vpn_range_is_used, PageTableEntry};
pub use page_table::{PTEFlags, PageTable};

/// initiate heap allocator, frame allocator and kernel space
pub fn init() {
    heap_allocator::init_heap(); // 此处的heap是操作系统自己要用的（此处可以将操作系统作为整个电脑上的第一个应用程序，这个heap就是这个程序对应的heap）
    frame_allocator::init_frame_allocator(); //将整个物理内存在ekernel之后的空间都转化为frame
    KERNEL_SPACE.lock().activate();
}