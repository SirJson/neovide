use std::time::{Duration, Instant};
use std::thread::sleep;

use log::{info, trace, debug, error};
use skulpin::{LogicalSize, PhysicalSize};
use skulpin::sdl2;
use skulpin::sdl2::Sdl;
use skulpin::sdl2::video::Window;
use skulpin::sdl2::event::Event;
use skulpin::sdl2::keyboard::{Mod, Keycode};
use skulpin::{RendererBuilder, Renderer as SkulpinRenderer, PresentMode, CoordinateSystem, dpis};

use crate::settings::*;
use crate::bridge::{parse_keycode, append_modifiers, BRIDGE, UiCommand};
use crate::renderer::Renderer;
use crate::redraw_scheduler::REDRAW_SCHEDULER;
use crate::editor::EDITOR;
use crate::INITIAL_DIMENSIONS;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Asset;

#[cfg(target_os = "windows")]
fn windows_fix_dpi() {
    use winapi::um::winuser::SetProcessDpiAwarenessContext;
    use winapi::shared::windef::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn handle_new_grid_size(new_size: LogicalSize, renderer: &Renderer) {
    if new_size.width > 0 && new_size.height > 0 {
        let new_width = ((new_size.width + 1) as f32 / renderer.font_width) as u32;
        let new_height = ((new_size.height + 1) as f32 / renderer.font_height) as u32;
        // Add 1 here to make sure resizing doesn't change the grid size on startup
        BRIDGE.queue_command(UiCommand::Resize { width: new_width, height: new_height });
    }
}

struct WindowWrapper {
    context: Sdl,
    window: Window,
    skulpin_renderer: SkulpinRenderer,
    renderer: Renderer,
    mouse_down: bool,
    mouse_position: LogicalSize,
    title: String,
    previous_size: LogicalSize,
    previous_dpis: (f32, f32),
    ignore_text_input: bool
}

impl WindowWrapper {
    pub fn new() -> WindowWrapper {
        let context = sdl2::init().expect("Failed to initialize sdl2");
        let video_subsystem = context.video().expect("Failed to create sdl video subsystem");

        let (width, height) = INITIAL_DIMENSIONS;

        let renderer = Renderer::new();
        let logical_size = LogicalSize {
            width: (width as f32 * renderer.font_width) as u32, 
            height: (height as f32 * renderer.font_height + 1.0) as u32
        };
        
        #[cfg(target_os = "windows")]
        windows_fix_dpi();
        sdl2::hint::set("SDL_MOUSE_FOCUS_CLICKTHROUGH", "1");

        // let icon = {
        //     let icon_data = Asset::get("nvim.ico").expect("Failed to read icon data");
        //     let icon = load_from_memory(&icon_data).expect("Failed to parse icon data");
        //     let (width, height) = icon.dimensions();
        //     let mut rgba = Vec::with_capacity((width * height) as usize * 4);
        //     for (_, _, pixel) in icon.pixels() {
        //         rgba.extend_from_slice(&pixel.to_rgba().0);
        //     }
        //     Icon::from_rgba(rgba, width, height).expect("Failed to create icon object")
        // };
        // info!("icon created");

        let window = video_subsystem.window("Neovide", logical_size.width, logical_size.height)
                .position_centered()
                .allow_highdpi()
                .resizable()
                .vulkan()
                .build()
                .expect("Failed to create window");
        info!("window created");

        let skulpin_renderer = RendererBuilder::new()
            .prefer_integrated_gpu()
            .use_vulkan_debug_layer(true)
            .present_mode_priority(vec![PresentMode::Immediate])
            .coordinate_system(CoordinateSystem::Logical)
            .build(&window)
            .expect("Failed to create renderer");
        info!("renderer created");

        let previous_size = LogicalSize::new(&window).unwrap();
        let previous_dpis = dpis(&window).unwrap();

        WindowWrapper { 
            context, window, skulpin_renderer, renderer,
            mouse_down: false,
            mouse_position: LogicalSize {
                width: 0,
                height: 0
            },
            title: String::from("Neovide"),
            previous_size,
            previous_dpis,
            ignore_text_input: false
        }
    }

    pub fn synchronize_title(&mut self) {
        let editor_title = { EDITOR.lock().title.clone() };
        if self.title != editor_title {
            self.title = editor_title;
            self.window.set_title(&self.title).expect("Could not set title");
        }
    }

    pub fn handle_key_down(&mut self, keycode: Keycode, modifiers: Mod) {
        trace!("KeyDown Received: {}", keycode);
        if let Some((key_text, special)) = parse_keycode(keycode) {
            let will_text_input =
                !modifiers.contains(Mod::LCTRLMOD) &&
                !modifiers.contains(Mod::RCTRLMOD) &&
                !modifiers.contains(Mod::LALTMOD) &&
                !modifiers.contains(Mod::RALTMOD) &&
                !modifiers.contains(Mod::LGUIMOD) &&
                !modifiers.contains(Mod::RGUIMOD);
            if modifiers.contains(Mod::MODEMOD) || (will_text_input && !special) {
                self.ignore_text_input = false;
                return;
            }

            BRIDGE.queue_command(UiCommand::Keyboard(append_modifiers(modifiers, key_text, special)));
            self.ignore_text_input = true;
        }
    }

    pub fn handle_text_input(&mut self, text: String) {
        trace!("Keyboard Input Received: {}", &text);
        if self.ignore_text_input {
            self.ignore_text_input = false;
        } else {
            let text = if text == "<" {
                String::from("<lt>")
            } else {
                text
            };
            BRIDGE.queue_command(UiCommand::Keyboard(text))
        }
    }

    pub fn handle_pointer_motion(&mut self, x: i32, y: i32) {
        let previous_position = self.mouse_position;
        self.mouse_position = LogicalSize::from_physical_size_tuple((
                (x as f32 / self.renderer.font_width) as u32,
                (y as f32 / self.renderer.font_height) as u32
            ), 
            &self.window
        ).expect("Could not calculate logical mouse position");
        if self.mouse_down && previous_position != self.mouse_position {
            BRIDGE.queue_command(UiCommand::Drag(self.mouse_position.width, self.mouse_position.height));
        }
    }

    pub fn handle_pointer_down(&mut self) {
        BRIDGE.queue_command(UiCommand::MouseButton { 
            action: String::from("press"), 
            position: (self.mouse_position.width, self.mouse_position.height) 
        });
        self.mouse_down = true;
    }

    pub fn handle_pointer_up(&mut self)  {
        BRIDGE.queue_command(UiCommand::MouseButton { 
            action: String::from("release"), 
            position: (self.mouse_position.width, self.mouse_position.height) 
        });
        self.mouse_down = false;
    }

    pub fn handle_mouse_wheel(&mut self, x: i32, y: i32) {
        let vertical_input_type = if y > 0 {
            Some("up")
        } else if y < 0 {
            Some("down")
        } else {
            None
        };

        if let Some(input_type) = vertical_input_type {
            BRIDGE.queue_command(UiCommand::Scroll { 
                direction: input_type.to_string(), 
                position: (self.mouse_position.width, self.mouse_position.height) 
            });
        }

        let horizontal_input_type = if x > 0 {
            Some("right")
        } else if x < 0 {
            Some("left")
        } else {
            None
        };

        if let Some(input_type) = horizontal_input_type {
            BRIDGE.queue_command(UiCommand::Scroll { 
                direction: input_type.to_string(), 
                position: (self.mouse_position.width, self.mouse_position.height) 
            });
        }
    }

    pub fn draw_frame(&mut self) -> bool {
        let new_size = LogicalSize::new(&self.window).unwrap();
        if self.previous_size != new_size {
            handle_new_grid_size(new_size, &self.renderer);
            self.previous_size = new_size;
        }

        let new_dpis = dpis(&self.window).unwrap();
        if self.previous_dpis != new_dpis {
            let physical_size = PhysicalSize::new(&self.window);
            self.window.set_size(
                (physical_size.width as f32 * new_dpis.0 / self.previous_dpis.0) as u32,
                (physical_size.height as f32 * new_dpis.1 / self.previous_dpis.1) as u32).unwrap();
            self.previous_dpis = new_dpis;
        }

        debug!("Render Triggered");
        if REDRAW_SCHEDULER.should_draw() || SETTINGS.get::<WindowSettings>().no_idle {
            let renderer = &mut self.renderer;
            if self.skulpin_renderer.draw(&self.window, |canvas, coordinate_system_helper| {

                let dt = 1.0 / (SETTINGS.get::<WindowSettings>().refresh_rate as f32);

                if renderer.draw(canvas, coordinate_system_helper, dt) {
                    handle_new_grid_size(new_size, &renderer)
                }
            }).is_err() {
                error!("Render failed. Closing");
                return false;
            }
        }
        return true;
    }
}

#[derive(Clone)]
struct WindowSettings {
    refresh_rate: u64,
    no_idle: bool,
}

pub fn initialize_settings() {
    
    let no_idle = SETTINGS.neovim_arguments.contains(&String::from("--noIdle"));

    SETTINGS.set(&WindowSettings {
        refresh_rate: 60,
        no_idle,
    });
    
    register_nvim_setting!("refresh_rate", WindowSettings::refresh_rate);
    register_nvim_setting!("no_idle", WindowSettings::no_idle);
}

pub fn ui_loop() {
    let mut window = WindowWrapper::new();

    info!("Starting window event loop");
    let mut event_pump = window.context.event_pump().expect("Could not create sdl event pump");
    'running: loop {
        let frame_start = Instant::now();

        window.synchronize_title();

        for event in event_pump.poll_iter() {
            match event {
                Event::Quit {..} => break 'running,
                Event::Window {..} => REDRAW_SCHEDULER.queue_next_frame(),
                Event::KeyDown { keycode: Some(keycode), keymod: modifiers, .. } => window.handle_key_down(keycode, modifiers),
                Event::TextInput { text, .. } => window.handle_text_input(text),
                Event::MouseMotion { x, y, .. } => window.handle_pointer_motion(x, y),
                Event::MouseButtonDown { .. } => window.handle_pointer_down(),
                Event::MouseButtonUp { .. } => window.handle_pointer_up(),
                Event::MouseWheel { x, y, .. } => window.handle_mouse_wheel(x, y),
                _ => {}
            }
        }

        if !window.draw_frame() {
            break;
        }

        let elapsed = frame_start.elapsed();
        let refresh_rate = SETTINGS.get::<WindowSettings>().refresh_rate as f32;
        let frame_length = Duration::from_secs_f32(1.0 / refresh_rate);
        if elapsed < frame_length {
            sleep(frame_length - elapsed);
        }
    }
}
