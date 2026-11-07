//! Workaround for a VTE 0.76 (GTK4) font reference-counting bug that crashes
//! the app with SIGSEGV in `g_type_check_instance_is_fundamentally_a`.
//!
//! VTE 0.76 caches one `PangoFont` pointer per distinct glyph it has drawn,
//! but in its GTK4 code path it stores the pointer *without* taking a
//! reference (`FontInfo::get_unistr_info`, fonts-pangocairo.cc), while the
//! cache destructor does call `g_object_unref` on it. A font cache is freed
//! 30 s after no terminal uses it any more: after a zoom, a font change, or
//! closing a pane. Each freed cache therefore steals one reference per cached
//! glyph from fonts that Pango and GTK still use. After a few cycles a font is
//! finalized while still in use, and the next unref crashes the app.
//!
//! Upstream fixed this in VTE 0.77/0.78 by taking the missing reference.
//! Ubuntu 24.04 ships 0.76, so we apply the same fix at runtime: we redirect
//! only libvte's own imports of `pango_layout_get_line_readonly` and
//! `pango_glyph_string_copy` (its GOT slots), and take the reference exactly
//! where VTE stores the font. No other library's calls are affected, and
//! nothing is changed unless every check passes.

use gtk4::glib::gobject_ffi;
use gtk4::pango::ffi as pango_ffi;
use std::cell::Cell;
use std::ffi::{c_char, c_int, CStr};
use std::sync::OnceLock;

/// Outcome of [`install`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The loaded VTE has the bug and the workaround is active.
    Installed,
    /// The loaded VTE does not have the bug (major, minor).
    NotNeeded(u32, u32),
    /// The VTE has the bug but the workaround could not be applied.
    Failed(String),
}

/// Apply the workaround if the loaded VTE needs it. Idempotent; call before
/// any terminal draws.
pub fn install() -> Status {
    static STATUS: OnceLock<Status> = OnceLock::new();
    STATUS
        .get_or_init(|| {
            // SAFETY: plain version getters with no preconditions.
            let (major, minor) = unsafe { (vte4::ffi::vte_get_major_version(), vte4::ffi::vte_get_minor_version()) };
            if !has_bug(major, minor) {
                return Status::NotNeeded(major, minor);
            }
            match patch_libvte() {
                Ok(()) => Status::Installed,
                Err(e) => {
                    eprintln!("cmux-linux: VTE {major}.{minor} font crash workaround not applied: {e}");
                    Status::Failed(e)
                }
            }
        })
        .clone()
}

/// Only the 0.76 GTK4 code path stores the font without a reference
/// (0.74 and 0.77+ take one, so patching them would leak fonts).
fn has_bug(major: u32, minor: u32) -> bool {
    major == 0 && minor == 76
}

thread_local! {
    /// The line VTE most recently fetched from its layout; consumed by the
    /// next `pango_glyph_string_copy` from libvte.
    static LAST_LINE: Cell<*mut pango_ffi::PangoLayoutLine> = const { Cell::new(std::ptr::null_mut()) };
}

unsafe extern "C" fn get_line_readonly_hook(layout: *mut pango_ffi::PangoLayout, line: c_int) -> *mut pango_ffi::PangoLayoutLine {
    // Calls from this binary go through our own, unpatched GOT.
    let result = pango_ffi::pango_layout_get_line_readonly(layout, line);
    LAST_LINE.with(|l| l.set(result));
    result
}

unsafe extern "C" fn glyph_string_copy_hook(glyphs: *mut pango_ffi::PangoGlyphString) -> *mut pango_ffi::PangoGlyphString {
    let line = LAST_LINE.with(|l| l.replace(std::ptr::null_mut()));
    // VTE 0.76 does, in FontInfo::get_unistr_info():
    //   line = pango_layout_get_line_readonly(m_layout, 0);
    //   glyph_item = line->runs->data;
    //   ufi->font = glyph_item->item->analysis.font;          // missing ref
    //   ufi->glyph_string = pango_glyph_string_copy(glyph_item->glyphs);
    // Take the reference only when the copied string is exactly that run's.
    if !glyphs.is_null() && !line.is_null() {
        let runs = (*line).runs;
        if !runs.is_null() {
            let run = (*runs).data as *mut pango_ffi::PangoGlyphItem;
            if !run.is_null() && (*run).glyphs == glyphs && !(*run).item.is_null() {
                let font = (*(*run).item).analysis.font;
                if !font.is_null() {
                    gobject_ffi::g_object_ref(font.cast());
                }
            }
        }
    }
    pango_ffi::pango_glyph_string_copy(glyphs)
}

// ---- GOT patching ---------------------------------------------------------

#[cfg(target_arch = "x86_64")]
const R_JUMP_SLOT: u32 = 7;
#[cfg(target_arch = "x86_64")]
const R_GLOB_DAT: u32 = 6;
#[cfg(target_arch = "aarch64")]
const R_JUMP_SLOT: u32 = 1026;
#[cfg(target_arch = "aarch64")]
const R_GLOB_DAT: u32 = 1025;

const DT_NULL: i64 = 0;
const DT_PLTRELSZ: i64 = 2;
const DT_STRTAB: i64 = 5;
const DT_SYMTAB: i64 = 6;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_JMPREL: i64 = 23;
const PT_DYNAMIC: u32 = 2;
const PT_GNU_RELRO: u32 = 0x6474_e552;

#[repr(C)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,
}

#[repr(C)]
#[allow(dead_code)] // mirrors the ELF layout
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

#[repr(C)]
#[allow(dead_code)] // mirrors the ELF layout
struct Elf64Sym {
    st_name: u32,
    st_info: u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size: u64,
}

struct Library {
    base: usize,
    dynamic: *const Elf64Dyn,
    relro: Option<(usize, usize)>,
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn patch_libvte() -> Result<(), String> {
    Err("unsupported architecture".into())
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn patch_libvte() -> Result<(), String> {
    let lib = find_libvte().ok_or("libvte-2.91-gtk4 is not loaded")?;
    // SAFETY: `lib` describes a loaded, relocated ELF object of this process;
    // the dynamic linker keeps its tables mapped for the process lifetime.
    unsafe {
        let line_slot = got_slot(&lib, c"pango_layout_get_line_readonly")?;
        let copy_slot = got_slot(&lib, c"pango_glyph_string_copy")?;
        // Both or nothing: one hook without the other would be pointless.
        write_slot(&lib, line_slot, get_line_readonly_hook as *const () as usize)?;
        write_slot(&lib, copy_slot, glyph_string_copy_hook as *const () as usize)?;
    }
    Ok(())
}

fn find_libvte() -> Option<Library> {
    unsafe extern "C" fn visit(info: *mut libc::dl_phdr_info, _size: libc::size_t, data: *mut libc::c_void) -> c_int {
        let info = &*info;
        if info.dlpi_name.is_null() {
            return 0;
        }
        let name = CStr::from_ptr(info.dlpi_name).to_bytes();
        let file = name.rsplit(|&b| b == b'/').next().unwrap_or(name);
        if !file.starts_with(b"libvte-2.91-gtk4.so") {
            return 0;
        }
        let base = info.dlpi_addr as usize;
        let mut dynamic = std::ptr::null();
        let mut relro = None;
        for i in 0..info.dlpi_phnum as usize {
            let ph = &*info.dlpi_phdr.add(i);
            match ph.p_type {
                PT_DYNAMIC => dynamic = (base + ph.p_vaddr as usize) as *const Elf64Dyn,
                PT_GNU_RELRO => relro = Some((base + ph.p_vaddr as usize, ph.p_memsz as usize)),
                _ => {}
            }
        }
        if dynamic.is_null() {
            return 0;
        }
        *(data as *mut Option<Library>) = Some(Library { base, dynamic, relro });
        1
    }
    let mut found: Option<Library> = None;
    // SAFETY: the callback only reads the loader-provided info and writes
    // through `data`, which points at `found` for the duration of the call.
    unsafe { libc::dl_iterate_phdr(Some(visit), (&mut found as *mut Option<Library>).cast()) };
    found
}

/// Address of the GOT entry through which libvte calls `symbol`.
unsafe fn got_slot(lib: &Library, symbol: &CStr) -> Result<*mut usize, String> {
    // glibc rewrites these entries to absolute addresses on most targets;
    // values below the load base are still file-relative.
    let abs = |v: u64| -> usize {
        let v = v as usize;
        if v < lib.base {
            lib.base + v
        } else {
            v
        }
    };
    let (mut strtab, mut symtab) = (0usize, 0usize);
    let (mut jmprel, mut jmprel_size, mut rela, mut rela_size) = (0usize, 0usize, 0usize, 0usize);
    let mut d = lib.dynamic;
    while (*d).d_tag != DT_NULL {
        let v = (*d).d_val;
        match (*d).d_tag {
            DT_STRTAB => strtab = abs(v),
            DT_SYMTAB => symtab = abs(v),
            DT_JMPREL => jmprel = abs(v),
            DT_PLTRELSZ => jmprel_size = v as usize,
            DT_RELA => rela = abs(v),
            DT_RELASZ => rela_size = v as usize,
            _ => {}
        }
        d = d.add(1);
    }
    if strtab == 0 || symtab == 0 {
        return Err("libvte has no dynamic symbol table".into());
    }
    for (table, size) in [(jmprel, jmprel_size), (rela, rela_size)] {
        if table == 0 {
            continue;
        }
        let relocs = std::slice::from_raw_parts(table as *const Elf64Rela, size / std::mem::size_of::<Elf64Rela>());
        for r in relocs {
            let kind = (r.r_info & 0xffff_ffff) as u32;
            if kind != R_JUMP_SLOT && kind != R_GLOB_DAT {
                continue;
            }
            let sym = &*((symtab as *const Elf64Sym).add((r.r_info >> 32) as usize));
            if CStr::from_ptr((strtab + sym.st_name as usize) as *const c_char) == symbol {
                return Ok((lib.base + r.r_offset as usize) as *mut usize);
            }
        }
    }
    Err(format!("libvte does not import {}", symbol.to_string_lossy()))
}

/// Point a GOT entry at `target`, temporarily unprotecting RELRO memory.
unsafe fn write_slot(lib: &Library, slot: *mut usize, target: usize) -> Result<(), String> {
    let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
    let start = slot as usize & !(page - 1);
    let in_relro = lib.relro.is_some_and(|(at, len)| (at..at + len).contains(&(slot as usize)));
    if libc::mprotect(start as *mut libc::c_void, page, libc::PROT_READ | libc::PROT_WRITE) != 0 {
        return Err(format!("mprotect: {}", std::io::Error::last_os_error()));
    }
    std::ptr::write_volatile(slot, target);
    if in_relro {
        libc::mprotect(start as *mut libc::c_void, page, libc::PROT_READ);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_vte_0_76_needs_the_workaround() {
        assert!(has_bug(0, 76));
        for (major, minor) in [(0, 74), (0, 75), (0, 77), (0, 78), (0, 80), (1, 76)] {
            assert!(!has_bug(major, minor), "{major}.{minor}");
        }
    }

    #[test]
    fn finds_libvte_imports() {
        // Referencing VTE keeps it linked into this test binary.
        // SAFETY: plain version getter.
        assert_eq!(unsafe { vte4::ffi::vte_get_major_version() }, 0);
        let lib = find_libvte().expect("libvte is loaded");
        // SAFETY: lib describes the loaded libvte.
        unsafe {
            for sym in [c"pango_layout_get_line_readonly", c"pango_glyph_string_copy"] {
                let slot = got_slot(&lib, sym).unwrap();
                assert!(!slot.is_null());
            }
            assert!(got_slot(&lib, c"definitely_not_imported_by_vte").is_err());
        }
    }
}
