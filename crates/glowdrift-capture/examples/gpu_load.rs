//! Keeps the default GPU busy with large clears and mip generations, to measure how capture cost
//! behaves while other processes compete for the GPU.
//!
//! Usage: `gpu_load [--seconds <n>] [--size <px>]`

use std::time::{Duration, Instant};

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

fn main() {
    let mut seconds = 10.0f32;
    let mut size = 8192u32;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().unwrap_or_default();
        match flag.as_str() {
            "--seconds" => seconds = value.parse().unwrap_or(seconds),
            "--size" => size = value.parse().unwrap_or(size),
            _ => {}
        }
    }

    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: out pointers refer to live locals.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .expect("device");
    let (device, context) = (device.expect("device"), context.expect("context"));

    let desc = D3D11_TEXTURE2D_DESC {
        Width: size,
        Height: size,
        MipLevels: 0,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: `desc` and the out pointer refer to live locals.
    unsafe { device.CreateTexture2D(&raw const desc, None, Some(&raw mut texture)) }
        .expect("texture");
    let texture = texture.expect("texture");
    let mut rtv: Option<ID3D11RenderTargetView> = None;
    let mut srv: Option<ID3D11ShaderResourceView> = None;
    // SAFETY: default views on a live texture.
    unsafe {
        device
            .CreateRenderTargetView(&texture, None, Some(&raw mut rtv))
            .expect("rtv");
        device
            .CreateShaderResourceView(&texture, None, Some(&raw mut srv))
            .expect("srv");
    }
    let (rtv, srv) = (rtv.expect("rtv"), srv.expect("srv"));

    let end = Instant::now() + Duration::from_secs_f32(seconds);
    let mut rounds = 0u64;
    let mut shade = 0.0f32;
    while Instant::now() < end {
        shade = (shade + 0.13) % 1.0;
        // SAFETY: live views on the device's own texture.
        unsafe {
            context.ClearRenderTargetView(&rtv, &[shade, 1.0 - shade, 0.5, 1.0]);
            context.GenerateMips(&srv);
            context.Flush();
        }
        rounds += 1;
        // Bound the queue depth so the driver does not buffer an unbounded amount of work.
        if rounds.is_multiple_of(4) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    println!("{rounds} rounds on a {size}x{size} surface");
}
