//! Windows screen capture via WGC (ADR-0006).
//!
//! Direct3D11 + Windows.Graphics.Capture (`Direct3D11CaptureFramePool` +
//! `FrameArrived`), per Cap's `scap-direct3d`. Frames are BGRA8 with QPC
//! timestamps (`SystemRelativeTime`), ready for the master timeline (ADR-0003).

use super::{CaptureTarget, ScreenCapture, VideoFrame};
use crate::capture::clock::RawTimestamp;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use windows::{
    core::{HSTRING, Interface},
    Graphics::Capture::{
        Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
        GraphicsCaptureSession,
    },
    Graphics::DirectX::Direct3D11::IDirect3DDevice,
    Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Multithread, ID3D11Texture2D,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                DXGI_ERROR_UNSUPPORTED, IDXGIDevice,
            },
            Gdi::{EnumDisplayMonitors, GetMonitorInfoW, MONITORINFOEXW, HDC, HMONITOR},
        },
        System::{
            Com::{CoInitializeEx, COINIT},
            Performance::QueryPerformanceCounter,
            WinRT::Direct3D11::{
                CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
            },
            WinRT::Graphics::Capture::IGraphicsCaptureItemInterop,
        },
        UI::WindowsAndMessaging::MONITORINFOF_PRIMARY,
    },
};

const PIXEL_FORMAT_DXGI: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT =
    DXGI_FORMAT_B8G8R8A8_UNORM;

/// Owns the capture session + frame pool; runs one frame-delivery thread.
pub struct WindowsScreenCapture {
    target: CaptureTarget,
    max_fps: u32,
    stop: Arc<AtomicBool>,
    session: Option<GraphicsCaptureSession>,
    frame_pool: Option<Direct3D11CaptureFramePool>,
    poll_handle: Option<std::thread::JoinHandle<()>>,
    frame_counter: Arc<AtomicU64>,
    drop_counter: Arc<AtomicU64>,
}

impl WindowsScreenCapture {
    pub fn new(target: CaptureTarget, max_fps: u32) -> Self {
        Self {
            target,
            max_fps: max_fps.max(1),
            stop: Arc::new(AtomicBool::new(false)),
            session: None,
            frame_pool: None,
            poll_handle: None,
            frame_counter: Arc::new(AtomicU64::new(0)),
            drop_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn frame_counter(&self) -> u64 {
        self.frame_counter.load(Ordering::Relaxed)
    }
    pub fn drop_counter(&self) -> u64 {
        self.drop_counter.load(Ordering::Relaxed)
    }
}

fn qpc_now() -> i64 {
    let mut v: i64 = 0;
    unsafe { QueryPerformanceCounter(&mut v) }.unwrap_or_default();
    v
}

fn check_supported() -> Result<(), String> {
    use windows::Foundation::Metadata::ApiInformation;
    let contract = HSTRING::from("Windows.Foundation.UniversalApiContract");
    if !ApiInformation::IsApiContractPresentByMajor(&contract, 8).unwrap_or(false) {
        return Err("Windows version too old (needs Windows 10 1809+) for screen capture".into());
    }
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        return Err("Windows.Graphics.Capture is not supported on this system".into());
    }
    Ok(())
}

fn create_d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    let mut device: Option<ID3D11Device> = None;

    let mut result = unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            flags,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    };

    if let Err(e) = &result {
        if e.code() == DXGI_ERROR_UNSUPPORTED {
            device = None;
            result = unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    Default::default(),
                    flags,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )
            };
        }
    }
    result.map_err(|e| format!("D3D11CreateDevice failed: {e}"))?;

    let device = device.ok_or("D3D11CreateDevice returned no device")?;
    let context = unsafe { device.GetImmediateContext() }
        .map_err(|e| format!("GetImmediateContext failed: {e}"))?;

    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        unsafe { let _ = mt.SetMultithreadProtected(true); }
    }

    Ok((device, context))
}

fn to_direct3d_device(device: &ID3D11Device) -> Result<IDirect3DDevice, String> {
    let dxgi: IDXGIDevice = device
        .cast()
        .map_err(|e| format!("cast IDXGIDevice: {e}"))?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice: {e}"))?;
    inspectable
        .cast()
        .map_err(|e| format!("cast IDirect3DDevice: {e}"))
}

fn capture_item_for(target: &CaptureTarget) -> Result<GraphicsCaptureItem, String> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .map_err(|e| format!("factory IGraphicsCaptureItemInterop: {e}"))?;
    match target {
        CaptureTarget::Display(handle) => {
            let hmon = HMONITOR(*handle as *mut _);
            unsafe { interop.CreateForMonitor(hmon) }
                .map_err(|e| format!("CreateForMonitor: {e}"))
        }
        CaptureTarget::Window(hwnd) => {
            let hwnd = windows::Win32::Foundation::HWND(*hwnd as *mut _);
            unsafe { interop.CreateForWindow(hwnd) }
                .map_err(|e| format!("CreateForWindow: {e}"))
        }
    }
}

fn read_frame_to_bgra(
    frame: &Direct3D11CaptureFrame,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
) -> Result<VideoFrame, String> {
    let size = frame.ContentSize().map_err(|e| format!("ContentSize: {e}"))?;
    let width = size.Width as u32;
    let height = size.Height as u32;

    let surface = frame.Surface().map_err(|e| format!("Surface: {e}"))?;
    let dxgi_access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|e| format!("cast surface: {e}"))?;
    let texture: ID3D11Texture2D = unsafe { dxgi_access.GetInterface() }
        .map_err(|e| format!("GetInterface texture: {e}"))?;

    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: PIXEL_FORMAT_DXGI as _,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }
        .map_err(|e| format!("CreateTexture2D staging: {e}"))?;
    let staging = staging.ok_or("no staging texture")?;

    unsafe { context.CopyResource(&staging, &texture) };

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
        .map_err(|e| format!("Map: {e}"))?;

    let src = unsafe {
        std::slice::from_raw_parts(mapped.pData as *const u8, (mapped.RowPitch * height) as usize)
    };

    let mut data = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        let row_src = &src[(y as usize * mapped.RowPitch as usize)..];
        let dst = &mut data[(y as usize * width as usize * 4)..];
        dst[..(width as usize * 4)]
            .copy_from_slice(&row_src[..(width as usize * 4)]);
    }

    unsafe { context.Unmap(&staging, 0) };

    let ts = frame
        .SystemRelativeTime()
        .map_err(|e| format!("SystemRelativeTime: {e}"))?;

    Ok(VideoFrame {
        width,
        height,
        data,
        timestamp: RawTimestamp::from_qpc(ts.Duration),
    })
}

#[async_trait::async_trait]
impl ScreenCapture for WindowsScreenCapture {
    async fn start(
        &mut self,
        tx: tokio::sync::mpsc::Sender<VideoFrame>,
    ) -> Result<(), String> {
        if self.session.is_some() {
            return Err("capture already running".into());
        }

        check_supported()?;
        let _ = unsafe { CoInitializeEx(None, COINIT(2)) }; // COINIT_APARTMENTTHREADED

        let (device, context) = create_d3d_device()?;
        let d3d = to_direct3d_device(&device)?;
        let item = capture_item_for(&self.target)?;

        let size = item.Size().map_err(|e| format!("item.Size: {e}"))?;

        let pool_size: i32 = 2;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d,
            windows::Graphics::DirectX::DirectXPixelFormat::B8G8R8A8UIntNormalized,
            pool_size,
            size,
        )
        .map_err(|e| format!("CreateFreeThreaded: {e}"))?;

        let session = frame_pool
            .CreateCaptureSession(&item)
            .map_err(|e| format!("CreateCaptureSession: {e}"))?;

        // Cap frame rate (session-level min update interval)
        let fps = self.max_fps.min(60);
        let interval_ns = (1_000_000_000i64 / fps as i64) as u64;
        let _ = session.SetMinUpdateInterval(windows::Foundation::TimeSpan {
            Duration: interval_ns as i64,
        });

        let stop = self.stop.clone();
        let frame_counter = self.frame_counter.clone();
        let drop_counter = self.drop_counter.clone();
        let device_cb = device.clone();
        let context_cb = context.clone();
        let frame_pool_cb = frame_pool.clone();
        let ticks_per_frame = qpc_frequency() / fps as i64;

        // Poll the pool from a dedicated thread (works reliably headless; WGC
        // FreeThreaded pools allow TryGetNextFrame from any thread).
        let handle = std::thread::spawn(move || {
            // COM init on this thread (WGC FreeThreaded still likes it)
            let _ = unsafe { CoInitializeEx(None, COINIT(2)) };
            let mut last_sent_qpc = 0i64;
            let mut callbacks: u64 = 0;
            while !stop.load(Ordering::Relaxed) {
                match frame_pool_cb.TryGetNextFrame() {
                    Ok(frame) => {
                        callbacks += 1;
                        let now_qpc = qpc_now();
                        if now_qpc.saturating_sub(last_sent_qpc) >= ticks_per_frame {
                            match read_frame_to_bgra(&frame, &device_cb, &context_cb) {
                                Ok(vf) => {
                                    last_sent_qpc = now_qpc;
                                    frame_counter.fetch_add(1, Ordering::Relaxed);
                                    if tx.blocking_send(vf).is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[capture] frame read error: {e}");
                                    drop_counter.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        } else {
                            drop_counter.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        // No new frame yet (E_BOUNDS/E_PENDING); poll again.
                        let code = e.code().0 as u32;
                        if code == 0x80000000 || code == 0x80070057 {
                            // ignore normal "no frame" errors
                        } else if callbacks < 5 {
                            eprintln!("[capture] TryGetNextFrame err: {e:?}");
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let _ = frame_pool_cb.Close();
            eprintln!("[capture] poll thread exited (frames={})", frame_counter.load(Ordering::Relaxed));
        });

        session.StartCapture().map_err(|e| format!("session.Start: {e}"))?;

        self.session = Some(session);
        self.frame_pool = Some(frame_pool);
        self.poll_handle = Some(handle);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self.session.take() {
            s.Close().ok();
        }
        if let Some(h) = self.poll_handle.take() {
            let _ = h.join();
        }
        if let Some(p) = self.frame_pool.take() {
            p.Close().ok();
        }
        Ok(())
    }
}

fn qpc_frequency() -> i64 {
    use std::sync::OnceLock;
    use windows::Win32::System::Performance::QueryPerformanceFrequency;
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f: i64 = 0;
        unsafe { QueryPerformanceFrequency(&mut f) }.unwrap_or_default();
        f
    })
}

// ---------- target enumeration ----------

/// Enumerate displays (HMONITOR) with friendly names + physical size.
/// Returns (target, label, width, height).
pub fn list_windows_capture_targets() -> Vec<(CaptureTarget, String, u32, u32)> {
    let mut out = Vec::new();
    let mut list: Vec<HMONITOR> = Vec::new();

    unsafe extern "system" fn enum_proc(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _lprc: *mut RECT,
        lparam: LPARAM,
    ) -> windows::core::BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HMONITOR>) };
        list.push(hmonitor);
        windows::core::BOOL(1)
    }

    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(std::ptr::addr_of_mut!(list) as isize),
        );
    }

    for (i, hmon) in list.iter().enumerate() {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let ok = unsafe { GetMonitorInfoW(*hmon, &mut info as *mut _ as *mut _) }.as_bool();
        if !ok {
            continue;
        }
        let rc = info.monitorInfo.rcMonitor;
        let w = (rc.right - rc.left) as u32;
        let h = (rc.bottom - rc.top) as u32;
        let is_primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;
        let label = format!(
            "Display {} ({w}x{h}){}",
            i,
            if is_primary { " [Primary]" } else { "" }
        );
        out.push((CaptureTarget::Display(hmon.0 as u64), label, w, h));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_displays_finds_at_least_one() {
        let targets = list_windows_capture_targets();
        assert!(!targets.is_empty(), "expected at least one display");
        let (target, label, w, h) = &targets[0];
        assert!(w > &0 && h > &0, "display size invalid: {label}");
        assert!(matches!(target, CaptureTarget::Display(_)));
    }
}
