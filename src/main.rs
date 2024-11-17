use drm::buffer::{Buffer, DrmFourcc};
use drm::control::{connector, crtc, plane, Device as _, ResourceHandles};
use drm::Device;
use eyre::{bail, Result};
use image::Rgba;
use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::{AsFd, BorrowedFd},
    path::Path,
};

fn display<P: AsRef<Path>>(path: P) -> Result<()> {
    let Some(card) = Card::find_device() else {
        bail!("Failed to open any card, terminating")
    };
    // Make sure we have master
    card.acquire_master_lock()?;
    let resources = card.resource_handles()?;
    let Some(connector) = resources.connectors().iter().find_map(|&handle| {
        let connector = card.get_connector(handle, false).ok()?;
        (connector.state() == connector::State::Connected).then_some(connector)
    }) else {
        bail!("Failed to find any connected output");
    };
    let encoder = card.get_encoder(connector.current_encoder().unwrap())?;
    let crtc = card.get_crtc(encoder.crtc().unwrap())?;
    let plane = card.get_crtc_plane(&resources, crtc.handle())?;
    if !plane
        .formats()
        .iter()
        .copied()
        .any(|f| f == (DrmFourcc::Argb8888 as u32))
    {
        bail!("Failed to find suitable format in plane.");
    }
    let picture = image::open(path).unwrap().into_rgba8();
    let mut buffer = card.create_dumb_buffer(picture.dimensions(), DrmFourcc::Argb8888, 32)?;
    let buffer_size = buffer.size();
    {
        let pitch = buffer.pitch();
        let mut mapping = card.map_dumb_buffer(&mut buffer)?;
        for (x, y, &Rgba([r, g, b, a])) in picture.enumerate_pixels() {
            if x >= buffer_size.0 {
                continue;
            }
            if y >= buffer_size.1 {
                break;
            }
            let index = x as usize * 4 + y as usize * pitch as usize;
            // Note: Argb8888 is always little-endian, even on big-endian architectures
            mapping[index + 3] = a;
            mapping[index + 2] = r;
            mapping[index + 1] = g;
            mapping[index + 0] = b;
        }
    }
    let framebuffer = card.add_framebuffer(&buffer, 32, 32)?;
    card.set_plane(
        plane.handle(),
        crtc.handle(),
        Some(framebuffer),
        0,
        (0, 0, buffer.size().0, buffer.size().1),
        (0, 0, buffer.size().0 << 16, buffer.size().1 << 16),
    )?;
    card.release_master_lock()?;
    eprintln!("Ctrl+C to quit");
    loop {
        std::thread::park();
    }
}

fn main() -> Result<()> {
    if let Some(path) = std::env::args_os().nth(1) {
        display(path)?;
    } else {
        bail!("Please provide the path to an image as an argument.");
    }
    Ok(())
}

struct Card(File);

impl Card {
    fn find_device() -> Option<Card> {
        (0..=255).find_map(|i| {
            let path = format!("/dev/dri/card{i}");
            Card::open(&path)
                .inspect_err(|e| {
                    if e.kind() != io::ErrorKind::NotFound {
                        eprintln!("Failed to open {path}: {e:?}");
                    }
                })
                .ok()
        })
    }

    fn open(path: impl AsRef<Path>) -> io::Result<Card> {
        Ok(Card(OpenOptions::new().read(true).write(true).open(&path)?))
    }

    fn get_crtc_plane(
        &self,
        resources: &ResourceHandles,
        crtc: crtc::Handle,
    ) -> Result<plane::Info> {
        for handle in self.plane_handles()? {
            let plane = self.get_plane(handle)?;
            if plane.crtc().is_none()
                && resources
                    .filter_crtcs(plane.possible_crtcs())
                    .contains(&crtc)
            {
                return Ok(plane);
            }
        }
        bail!("Failed to find a suitable plane for crtc");
    }
}

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl drm::Device for Card {}
impl drm::control::Device for Card {}
