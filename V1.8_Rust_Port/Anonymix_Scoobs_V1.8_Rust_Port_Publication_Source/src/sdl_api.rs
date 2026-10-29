//! Exact dynamically loaded SDL APIs. All operations and destruction stay on
//! the main/UI thread; Rc-owned handles keep the DLL loaded until resources die.
use crate::{native_library::NativeLibrary, sdl_abi::*, ui_geometry::Rect};
use std::{
    ffi::{c_char, CStr},
    path::Path,
};
macro_rules! api {
 ($name:ident {$($field:ident:$signature:ty=>$symbol:literal),+ $(,)?})=>{
  /// Fixed C function table retaining its dynamically loaded library on the UI thread.
  pub struct $name {$(pub $field:$signature,)+ _module:NativeLibrary}
  impl $name {
   /// Resolve all fixed exports or release the partial library and return its loader error.
   pub fn load(path:&Path)->Result<Self,String>{let module=NativeLibrary::open(path)?;Ok(Self{$(
    // SAFETY: fixed official C export/signature; see SDL_ABI_REFERENCE.json.
    $field:unsafe{std::mem::transmute::<*mut std::ffi::c_void,$signature>(module.symbol(concat!($symbol,"\0").as_bytes())?)},
   )+_module:module})}
  }
 }
}
api! {Sdl {
 set_main_ready:Quit=>"SDL_SetMainReady",
 enter_callbacks:EnterCallbacks=>"SDL_EnterAppMainCallbacks",
 init:Init=>"SDL_Init", quit:Quit=>"SDL_Quit",
 error:unsafe extern "C" fn()->*const c_char=>"SDL_GetError",
 version:unsafe extern "C" fn()->i32=>"SDL_GetVersion",
 create_window:CreateWindow=>"SDL_CreateWindowAndRenderer",
 destroy_window:unsafe extern "C" fn(Handle)->()=>"SDL_DestroyWindow",
 destroy_renderer:unsafe extern "C" fn(Handle)->()=>"SDL_DestroyRenderer",
 window_size:unsafe extern "C" fn(Handle,*mut i32,*mut i32)->bool=>"SDL_GetWindowSize",
 minimum_size:unsafe extern "C" fn(Handle,i32,i32)->bool=>"SDL_SetWindowMinimumSize",
 set_window_size:unsafe extern "C" fn(Handle,i32,i32)->bool=>"SDL_SetWindowSize",
 primary_display:unsafe extern "C" fn()->u32=>"SDL_GetPrimaryDisplay",
 display_for_window:unsafe extern "C" fn(Handle)->u32=>"SDL_GetDisplayForWindow",
 usable_bounds:unsafe extern "C" fn(u32,*mut IntRect)->bool=>"SDL_GetDisplayUsableBounds",
 mouse:unsafe extern "C" fn(*mut f32,*mut f32)->u32=>"SDL_GetMouseState",
 ticks:unsafe extern "C" fn()->u64=>"SDL_GetTicks", ticks_ns:unsafe extern "C" fn()->u64=>"SDL_GetTicksNS",
 delay_precise:unsafe extern "C" fn(u64)->()=>"SDL_DelayPrecise",
 draw_color:unsafe extern "C" fn(Handle,f32,f32,f32,f32)->bool=>"SDL_SetRenderDrawColorFloat",
 draw_blend:unsafe extern "C" fn(Handle,u32)->bool=>"SDL_SetRenderDrawBlendMode",
 clear:unsafe extern "C" fn(Handle)->bool=>"SDL_RenderClear",
 fill_rect:unsafe extern "C" fn(Handle,*const Rect)->bool=>"SDL_RenderFillRect",
 lines:DrawPoints=>"SDL_RenderLines",
 line:unsafe extern "C" fn(Handle,f32,f32,f32,f32)->bool=>"SDL_RenderLine",
 geometry:Geometry=>"SDL_RenderGeometry",
 create_texture:unsafe extern "C" fn(Handle,u32,i32,i32,i32)->Handle=>"SDL_CreateTexture",
 destroy_texture:unsafe extern "C" fn(Handle)->()=>"SDL_DestroyTexture",
 texture_from_surface:unsafe extern "C" fn(Handle,*mut Surface)->Handle=>"SDL_CreateTextureFromSurface",
 texture_scale:unsafe extern "C" fn(Handle,i32)->bool=>"SDL_SetTextureScaleMode",
 texture_blend:unsafe extern "C" fn(Handle,u32)->bool=>"SDL_SetTextureBlendMode",
 set_target:unsafe extern "C" fn(Handle,Handle)->bool=>"SDL_SetRenderTarget",
 get_target:unsafe extern "C" fn(Handle)->Handle=>"SDL_GetRenderTarget",
 render_scale:unsafe extern "C" fn(Handle,f32,f32)->bool=>"SDL_SetRenderScale",
 render_viewport:unsafe extern "C" fn(Handle,*const IntRect)->bool=>"SDL_SetRenderViewport",
 output_size:unsafe extern "C" fn(Handle,*mut i32,*mut i32)->bool=>"SDL_GetRenderOutputSize",
 texture:DrawTexture=>"SDL_RenderTexture",
 present:unsafe extern "C" fn(Handle)->bool=>"SDL_RenderPresent",
 load_bmp:unsafe extern "C" fn(*const c_char)->*mut Surface=>"SDL_LoadBMP",
 destroy_surface:unsafe extern "C" fn(*mut Surface)->()=>"SDL_DestroySurface",
 window_icon:unsafe extern "C" fn(Handle,*mut Surface)->bool=>"SDL_SetWindowIcon"
}}
impl Sdl {
    /// Copy SDL's current error string before the next SDL call can replace it.
    pub fn error_text(&self) -> String {
        unsafe {
            let value = (self.error)();
            if value.is_null() {
                "unknown SDL error".into()
            } else {
                CStr::from_ptr(value).to_string_lossy().into_owned()
            }
        }
    }
}
api! {Ttf {
 init:unsafe extern "C" fn()->bool=>"TTF_Init",quit:Quit=>"TTF_Quit",
 open_font:unsafe extern "C" fn(*const c_char,f32)->Handle=>"TTF_OpenFont",
 close_font:unsafe extern "C" fn(Handle)->()=>"TTF_CloseFont",
 render:unsafe extern "C" fn(Handle,*const c_char,usize,TextColor)->*mut Surface=>"TTF_RenderText_Blended",
 size:unsafe extern "C" fn(Handle,*const c_char,usize,*mut i32,*mut i32)->bool=>"TTF_GetStringSize"
}}
