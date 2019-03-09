extern crate chrono;
extern crate clap;
extern crate clipboard;
extern crate colored;
extern crate repng;
extern crate reqwest;
extern crate scrap;
extern crate sdl2;

use std::cmp;
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;
use std::process::exit;
use std::thread;
use std::time::Duration;

use chrono::prelude::*;
use clap::{App, Arg};
use clipboard::{ClipboardContext, ClipboardProvider};
use colored::*;
use scrap::{Capturer, Display, Frame};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;

const AMEOTRACK_UPLOAD_URL: &str = "https://ameo.link/u/upload";

fn get_capturer() -> Capturer {
    let display = Display::primary().expect("Couldn't find primary display.");
    Capturer::new(display).expect("Couldn't begin capture.")
}

fn ameotrack_upload<P: AsRef<Path>>(
    filename: P,
    expiry: String,
    secret: bool,
    one_time: bool,
) -> Result<String, Box<Error>> {
    let password = env::var("AMEOTRACK_PASSWORD")
        .expect("The `AMEOTRACK_PASSWORD` environment variable must be set!");

    let body = reqwest::multipart::Form::new()
        .file("file", filename)?
        .text("secret", if secret { "1" } else { "" })
        .text("expiry", expiry)
        .text("password", password)
        .text("oneTime", if one_time { "1" } else { "" });

    let client = reqwest::Client::new();
    let mut res = client.post(AMEOTRACK_UPLOAD_URL).multipart(body).send()?;

    let res_text = res
        .text()
        .expect("Unable to parse HTTP response into text!");
    if !res.status().is_success() {
        println!("Error uploading image to AmeoTrack: {:?}", res_text);
        exit(1);
    }

    Ok(res_text)
}

pub fn main() {
    let matches = App::new("Snapmeo")
        .version("0.1.0")
        .author("Casey Primozic <me@ameo.link>")
        .about("Takes screenshots of regions of the screen and uploads them to AmeoTrack")
        .arg(
            Arg::with_name("output_dir")
                .short("o")
                .long("output_dir")
                .help("Directory into which screenshots will be saved")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("secret")
                .short("s")
                .long("secret")
                .help("If set, the filenames of uploaded files will be obfuscated.")
                .takes_value(false),
        )
        .arg(
            Arg::with_name("expiry")
                .short("e")
                .long("expiry")
                .help("How long the image will be hosted before deletion, in days")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("one-time")
                .short("b")
                .long("one-time")
                .help("If set, the image will be deleted as soon as it is viewed once.")
                .takes_value(false),
        )
        .get_matches();

    let local: DateTime<Local> = Local::now();
    let date_string = local.format("%b %m %H-%M-%S").to_string();
    let filename = format!("Screenshot at {}.png", date_string);
    let filename = Path::new(matches.value_of("output_dir").unwrap()).join(filename);

    // TODO: Parallelize with window creation + canvas setup
    let mut capturer = get_capturer();
    let one_second = Duration::new(1, 0);
    let one_frame = one_second / 60;

    let (width, height) = (capturer.width(), capturer.height());
    println!("{:?}", (width, height));

    loop {
        let frame: Frame = match capturer.frame() {
            Ok(buffer) => buffer,
            Err(error) => {
                if error.kind() == ErrorKind::WouldBlock {
                    // Keep spinning.
                    thread::sleep(one_frame);
                    continue;
                } else {
                    panic!("Error: {}", error);
                }
            }
        };
        // println!("Captured screenshot frame!");

        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();

        let window = video_subsystem
            .window("rust-sdl2 demo: Video", width as u32, height as u32)
            // .position_centered()
            .opengl()
            // .vulkan()
            .allow_highdpi()
            // .fullscreen_desktop()
            .borderless()
            .build()
            .unwrap();

        let mut canvas = window.into_canvas().build().unwrap();
        let texture_creator = canvas.texture_creator();
        // TODO: Pull this directly from the pixel buffer.  No reason not to.
        // let texture = texture_creator.load_texture("output.png").unwrap();
        let mut texture = texture_creator
            .create_texture_static(Some(PixelFormatEnum::ARGB8888), width as u32, height as u32)
            .expect("Unable to create texture!");
        texture
            .update(None, &*frame, width * 4)
            .expect("Error updating texture with image data!");

        // canvas.set_draw_color(Color::RGB(255, 0, 0));
        canvas.clear();
        canvas.copy(&texture, None, None).expect("Render failed");
        canvas.present();
        let mut event_pump = sdl_context.event_pump().unwrap();

        let mut rect_corner_1: (i32, i32) = (0, 0);

        let finish_screenshot =
            move |rect_corner_1: (i32, i32), rect_corner_2: (i32, i32)| -> Result<(), Box<Error>> {
                // println!("Corners: {:?}, {:?}", rect_corner_1, rect_corner_2);
                let rect_width = (rect_corner_1.0 - rect_corner_2.0).abs() as usize;
                let rect_height = (rect_corner_1.1 - rect_corner_2.1).abs() as usize;
                let min_x = cmp::min(rect_corner_1.0, rect_corner_2.0) as usize;
                let min_y = cmp::min(rect_corner_1.1, rect_corner_2.1) as usize;
                let mut flip_buffer: Vec<u8> = Vec::with_capacity(rect_width * rect_height * 4);
                let stride = width * 4;

                for y in 0..rect_height {
                    for x in 0..rect_width {
                        let i = (stride * (y + min_y)) + (4 * (x + min_x));
                        flip_buffer.extend_from_slice(&[frame[i + 2], frame[i + 1], frame[i], 255]);
                    }
                }

                let file = File::create(filename.clone()).expect("Unable to create output file!");

                repng::encode(file, rect_width as u32, rect_height as u32, &flip_buffer).unwrap();

                let expiry = matches.value_of("expiry").unwrap_or("-1");
                let secret = matches.is_present("secret");
                let one_time = matches.is_present("one-time");

                // Upload the image to AmeoTrack
                let image_url = ameotrack_upload(filename, expiry.to_owned(), secret, one_time)?;

                // Copy the URL to the clipboard and print to the console
                let mut ctx: ClipboardContext =
                    ClipboardProvider::new().expect("Unable to create clipboard context!");
                ctx.set_contents(image_url.clone())
                    .expect("Unable to set clipboard contents!");

                println!("{} {}", "File successfully uploaded:".green(), image_url);
                println!("Link has been copied to the clipboard.");

                Ok(())
            };

        'running: loop {
            for event in event_pump.poll_iter() {
                match event {
                    Event::Quit { .. }
                    | Event::KeyDown {
                        keycode: Some(Keycode::Escape),
                        ..
                    } => {
                        break 'running;
                    }
                    Event::MouseButtonDown { x, y, .. } => {
                        rect_corner_1 = (x, y);
                    }
                    Event::MouseButtonUp { x, y, .. } => {
                        match finish_screenshot(rect_corner_1, (x, y)) {
                            Ok(()) => (),
                            Err(err) => {
                                println!(
                                    "An error occured during the screenshotting and uploading process: {:?}",
                                    err
                                );
                            }
                        };
                        break 'running;
                    }
                    _ => {}
                }
            }
            thread::sleep(one_frame);
        }

        break;
    }
}
