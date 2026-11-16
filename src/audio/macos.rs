//! CoreAudio backend (macOS 14.2+).
//!
//! macOS has no per-application volume, so a process tap is used instead: the tap
//! mutes the application's own output, captures it, and our IO proc plays it back to
//! the default output device scaled by the requested gain. At 100% the tap is torn
//! down so an untouched app has zero overhead.

use super::{AudioApp, VolumeBackend};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, Message};
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::{size_of, MaybeUninit};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

const SYSTEM_OBJECT: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn get_value<T: Copy>(object: AudioObjectID, selector: AudioObjectPropertySelector) -> Option<T> {
    let addr = address(selector);
    let mut size = size_of::<T>() as u32;
    let mut value = MaybeUninit::<T>::zeroed();
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast()),
        )
    };
    (status == 0).then(|| unsafe { value.assume_init() })
}

fn get_ids(object: AudioObjectID, selector: AudioObjectPropertySelector) -> Vec<AudioObjectID> {
    let addr = address(selector);
    let mut size = 0u32;
    unsafe {
        if AudioObjectGetPropertyDataSize(object, NonNull::from(&addr), 0, std::ptr::null(), NonNull::from(&mut size))
            != 0
        {
            return Vec::new();
        }
        let mut ids = vec![0 as AudioObjectID; size as usize / size_of::<AudioObjectID>()];
        if ids.is_empty() {
            return ids;
        }
        let status = AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(ids.as_mut_ptr().cast()),
        );
        if status != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / size_of::<AudioObjectID>());
        ids
    }
}

/// Reads a CFString property; CoreAudio returns it retained (+1), which `from_raw` takes over.
fn get_string(object: AudioObjectID, selector: AudioObjectPropertySelector) -> Option<String> {
    let ptr: *mut NSString = get_value::<*mut NSString>(object, selector)?;
    let s = unsafe { Retained::from_raw(ptr) }?;
    Some(s.to_string())
}

fn as_any<T: Message>(obj: &Retained<T>) -> &AnyObject {
    unsafe { &*(Retained::as_ptr(obj) as *const AnyObject) }
}

fn dict(entries: &[(&str, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = entries.iter().map(|(k, _)| NSString::from_str(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = entries.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&key_refs, &values)
}

fn pid_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    String::from_utf8(buf).ok()
}

/// Display name from an executable path: the `.app` bundle name if there is one.
fn display_name(path: &str) -> String {
    if let Some(idx) = path.find(".app/") {
        let bundle = &path[..idx];
        return bundle.rsplit('/').next().unwrap_or(bundle).to_string();
    }
    path.rsplit('/').next().unwrap_or(path).to_string()
}

struct Process {
    object: AudioObjectID,
    key: String,
    title: String,
}

fn processes() -> Vec<Process> {
    let own_pid = std::process::id() as i32;
    let mut out = Vec::new();
    for object in get_ids(SYSTEM_OBJECT, kAudioHardwarePropertyProcessObjectList) {
        let Some(pid) = get_value::<i32>(object, kAudioProcessPropertyPID) else { continue };
        if pid == own_pid {
            continue;
        }
        let path = pid_path(pid).unwrap_or_default();
        let running = get_value::<u32>(object, kAudioProcessPropertyIsRunningOutput).unwrap_or(0) != 0;
        // Daemons register with coreaudiod too; only show apps and whatever is playing now.
        if !running && !path.contains(".app/") {
            continue;
        }
        let bundle = get_string(object, kAudioProcessPropertyBundleID).filter(|b| !b.is_empty());
        let title = display_name(&path);
        let key = bundle.unwrap_or_else(|| title.clone());
        if key.is_empty() {
            continue;
        }
        out.push(Process { object, key, title });
    }
    out
}

fn default_output_uid() -> Option<String> {
    let device = get_value::<AudioObjectID>(SYSTEM_OBJECT, kAudioHardwarePropertyDefaultOutputDevice)?;
    get_string(device, kAudioDevicePropertyDeviceUID)
}

/// Shared between the backend and the real-time IO thread.
struct IoState {
    gain: AtomicU32,
    last_gain: AtomicU32,
}

struct Tap {
    processes: Vec<AudioObjectID>,
    output_uid: String,
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    state: Box<IoState>,
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.aggregate_id, self.proc_id);
            AudioDeviceDestroyIOProcID(self.aggregate_id, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate_id);
            AudioHardwareDestroyProcessTap(self.tap_id);
        }
    }
}

unsafe fn buffers<'a>(list: NonNull<AudioBufferList>) -> &'a mut [AudioBuffer] {
    let list = list.as_ptr();
    let n = (*list).mNumberBuffers as usize;
    std::slice::from_raw_parts_mut(std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>(), n)
}

fn frames_of(bufs: &[AudioBuffer]) -> usize {
    match bufs.first() {
        Some(b) if bufs.len() == 1 => b.mDataByteSize as usize / 4 / (b.mNumberChannels.max(1) as usize),
        Some(b) => b.mDataByteSize as usize / 4,
        None => 0,
    }
}

/// Sample of `channel` at `frame` for both interleaved and planar float32 layouts.
unsafe fn sample(bufs: &[AudioBuffer], frame: usize, channel: usize) -> f32 {
    if bufs.len() == 1 {
        let b = &bufs[0];
        let nch = b.mNumberChannels.max(1) as usize;
        let data = b.mData as *const f32;
        if data.is_null() {
            return 0.0;
        }
        *data.add(frame * nch + channel.min(nch - 1))
    } else {
        let b = &bufs[channel.min(bufs.len() - 1)];
        let data = b.mData as *const f32;
        if data.is_null() {
            return 0.0;
        }
        *data.add(frame)
    }
}

unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let state = &*(client as *const IoState);
    let target = f32::from_bits(state.gain.load(Ordering::Relaxed));
    let start = f32::from_bits(state.last_gain.load(Ordering::Relaxed));
    state.last_gain.store(target.to_bits(), Ordering::Relaxed);

    let inp = buffers(input);
    let out = buffers(output);
    let in_frames = frames_of(inp);
    let planar_out = out.len() > 1;

    for (k, b) in out.iter_mut().enumerate() {
        let nch = b.mNumberChannels.max(1) as usize;
        let data = b.mData as *mut f32;
        if data.is_null() {
            continue;
        }
        let frames = b.mDataByteSize as usize / 4 / nch;
        for f in 0..frames {
            // Linear ramp over one buffer avoids clicks when the gain changes.
            let gain = start + (target - start) * (f as f32 / frames.max(1) as f32);
            for c in 0..nch {
                let channel = if planar_out { k } else { c };
                let v = if f < in_frames && channel < 2 { sample(inp, f, channel) * gain } else { 0.0 };
                *data.add(f * nch + c) = v;
            }
        }
    }
    0
}

impl Tap {
    fn create(processes: Vec<AudioObjectID>, output_uid: String, gain: f32) -> Result<Self, i32> {
        unsafe {
            let numbers: Vec<Retained<NSNumber>> = processes.iter().map(|p| NSNumber::new_u32(*p)).collect();
            let list = NSArray::from_retained_slice(&numbers);
            let desc = CATapDescription::initStereoMixdownOfProcesses(CATapDescription::alloc(), &list);
            desc.setMuteBehavior(CATapMuteBehavior::MutedWhenTapped);
            desc.setPrivate(true);
            desc.setName(&NSString::from_str("Apps Volume Control"));
            let tap_uuid = desc.UUID().UUIDString().to_string();

            let mut tap_id: AudioObjectID = 0;
            let status = AudioHardwareCreateProcessTap(Some(&desc), &mut tap_id);
            if status != 0 {
                return Err(status);
            }

            let aggregate_uid = NSUUID::new().UUIDString();
            let output_uid_ns = NSString::from_str(&output_uid);
            let yes = NSNumber::new_bool(true);
            let no = NSNumber::new_bool(false);
            let sub_device = dict(&[("uid", as_any(&output_uid_ns))]);
            let tap_entry = dict(&[("uid", as_any(&NSString::from_str(&tap_uuid))), ("drift", as_any(&yes))]);
            let sub_devices = NSArray::from_retained_slice(&[sub_device]);
            let taps = NSArray::from_retained_slice(&[tap_entry]);
            let name = NSString::from_str("Apps Volume Control");
            let description = dict(&[
                ("name", as_any(&name)),
                ("uid", as_any(&aggregate_uid)),
                ("master", as_any(&output_uid_ns)),
                ("private", as_any(&yes)),
                ("stacked", as_any(&no)),
                ("tapautostart", as_any(&yes)),
                ("subdevices", as_any(&sub_devices)),
                ("taps", as_any(&taps)),
            ]);

            let mut aggregate_id: AudioObjectID = 0;
            let cf = &*(Retained::as_ptr(&description) as *const CFDictionary);
            let status = AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate_id));
            if status != 0 {
                AudioHardwareDestroyProcessTap(tap_id);
                return Err(status);
            }

            let state =
                Box::new(IoState { gain: AtomicU32::new(gain.to_bits()), last_gain: AtomicU32::new(gain.to_bits()) });
            let mut proc_id: AudioDeviceIOProcID = None;
            let status = AudioDeviceCreateIOProcID(
                aggregate_id,
                Some(io_proc),
                &*state as *const IoState as *mut c_void,
                NonNull::from(&mut proc_id),
            );
            if status != 0 {
                AudioHardwareDestroyAggregateDevice(aggregate_id);
                AudioHardwareDestroyProcessTap(tap_id);
                return Err(status);
            }
            let tap = Tap { processes, output_uid, tap_id, aggregate_id, proc_id, state };
            let status = AudioDeviceStart(aggregate_id, proc_id);
            if status != 0 {
                return Err(status); // `tap` is dropped and cleaned up here
            }
            Ok(tap)
        }
    }

    fn set_gain(&self, gain: f32) {
        self.state.gain.store(gain.to_bits(), Ordering::Relaxed);
    }
}

pub struct Backend {
    /// Requested gain per app key; apps without an entry play at 100%.
    gains: HashMap<String, f32>,
    taps: HashMap<String, Tap>,
    procs: Vec<Process>,
}

impl Backend {
    pub fn new() -> Result<Self, String> {
        Ok(Self { gains: HashMap::new(), taps: HashMap::new(), procs: Vec::new() })
    }

    fn objects_for(&self, key: &str) -> Vec<AudioObjectID> {
        let mut ids: Vec<AudioObjectID> = self.procs.iter().filter(|p| p.key == key).map(|p| p.object).collect();
        ids.sort_unstable();
        ids
    }

    /// Creates, updates or removes the tap of `key` so that it matches `gains`.
    fn sync_tap(&mut self, key: &str) -> bool {
        let gain = self.gains.get(key).copied().unwrap_or(1.0);
        let objects = self.objects_for(key);
        if gain >= 0.999 || objects.is_empty() {
            self.taps.remove(key);
            return !objects.is_empty();
        }
        let Some(output) = default_output_uid() else { return false };
        if let Some(tap) = self.taps.get(key) {
            if tap.processes == objects && tap.output_uid == output {
                tap.set_gain(gain);
                return true;
            }
            self.taps.remove(key);
        }
        match Tap::create(objects, output, gain) {
            Ok(tap) => {
                self.taps.insert(key.to_string(), tap);
                true
            }
            Err(status) => {
                eprintln!("Failed to create audio tap for {key}: OSStatus {status}");
                false
            }
        }
    }
}

impl VolumeBackend for Backend {
    fn list(&mut self) -> Vec<AudioApp> {
        self.procs = processes();
        // Processes restart and the default output can change: keep taps in sync.
        let keys: Vec<String> = self.gains.keys().cloned().collect();
        for key in keys {
            self.sync_tap(&key);
        }
        let mut apps: Vec<AudioApp> = Vec::new();
        for p in &self.procs {
            if apps.iter().any(|a| a.key == p.key) {
                continue;
            }
            let volume = self.gains.get(&p.key).copied().unwrap_or(1.0);
            apps.push(AudioApp { key: p.key.clone(), title: p.title.clone(), volume, active: true });
        }
        apps.sort_by_key(|a| a.title.to_lowercase());
        apps
    }

    fn set_volume(&mut self, key: &str, volume: f32) -> bool {
        if volume >= 0.999 {
            self.gains.remove(key);
        } else {
            self.gains.insert(key.to_string(), volume);
        }
        if self.procs.is_empty() {
            self.procs = processes();
        }
        self.sync_tap(key)
    }
}
