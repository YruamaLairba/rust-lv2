//! Work scheduling library that allows real-time capable LV2 plugins to execute non-real-time actions.
//!
//! This crate allows plugins to schedule work that must be performed in another thread.
//! Plugins can use this interface to safely perform work that is not real-time safe, and receive
//! the result in the run context. A typical use case is a sampler reading and caching data from
//! disk. You can look at the
//! [LV2 Worker Specification](https://lv2plug.in/ns/ext/worker/worker.html) for more details.
//!
//! # WARNING!
//!
//! The current version of this crate have a **safety flow**. In the `work` method, you have a mutable
//! access to the plugin. Since the `work` method is meant to be called in another thread from the
//! run context, this allow **uncontrolled concurrent access** between two threads wich is **normally not
//! allowed** in safe rust. That means:
//!  * Change may occur in lv2-worker an lv2-crate to fix this issue.
//!  * It's up to you to ensure that no uncontrolled concurrent access happen.
//!
//! # Example
//!```
//!use core::any::Any;
//!use lv2_core::feature::*;
//!use lv2_core::prelude::*;
//!use lv2_worker::*;
//!
//!#[derive(PortCollection)]
//!struct Ports {}
//!
//!/// Requested features
//!#[derive(FeatureCollection)]
//!struct AudioFeatures<'a> {
//!    ///host feature allowing to schedule some work
//!    schedule: Schedule<'a, EgWorker>,
//!}
//!
//!//custom datatype
//!struct WorkMessage {
//!    cycle: usize,
//!    task: usize,
//!}
//!
//!/// A plugin that do some work in another thread
//!struct EgWorker {
//!    //schedule handler need to know the plugin type to use it use WorkData associated type.
//!    cycle: usize,
//!    end_cycle: usize,
//!}
//!
//!/// URI identifier
//!unsafe impl UriBound for EgWorker {
//!    const URI: &'static [u8] = b"urn:rust-lv2-more-examples:eg-worker-rs\0";
//!}
//!
//!impl Plugin for EgWorker {
//!    type Ports = Ports;
//!    type InitFeatures = ();
//!    type AudioFeatures = AudioFeatures<'static>;
//!
//!    fn new(_plugin_info: &PluginInfo, _features: &mut Self::InitFeatures) -> Option<Self> {
//!        Some(Self {
//!            cycle: 0,
//!            end_cycle: 1,
//!        })
//!    }
//!
//!    fn run(&mut self, _ports: &mut Ports, features: &mut Self::AudioFeatures) {
//!        self.cycle += 1;
//!        let cycle = self.cycle;
//!        println!("cycle {} started", cycle);
//!        for task in 0..10 {
//!            let work = WorkMessage { cycle, task };
//!            // schedule some work and passing some data
//!            let _ = features.schedule.schedule_work(work);
//!        }
//!    }
//!
//!    fn extension_data(uri: &Uri) -> Option<&'static dyn Any> {
//!        match_extensions![uri, WorkerDescriptor<Self>]
//!    }
//!}
//!
//!/// Implementing the extension.
//!impl Worker for EgWorker {
//!    // data type sent by the schedule handler and received by the `work` method.
//!    type WorkData = WorkMessage;
//!    // data type sent by the response handler and received by the `work_response` method.
//!    type ResponseData = String;
//!    fn work(
//!        &mut self,
//!        //response handler need to know the plugin type.
//!        response_handler: &ResponseHandler<Self>,
//!        data: Self::WorkData,
//!    ) -> Result<(), WorkerError> {
//!        println!("work received: cycle {}, task {}", data.cycle, data.task);
//!        if data.task >= 5 {
//!            let _ = response_handler.respond(format!( "response to cycle {}, task {}",
//!                data.cycle, data.task
//!            ));
//!        };
//!        Ok(())
//!    }
//!
//!    fn work_response(&mut self, data: Self::ResponseData, _features: &mut Self::AudioFeatures) -> Result<(), WorkerError> {
//!        println!("work_response received: {}", data);
//!        Ok(())
//!    }
//!
//!    fn end_run(&mut self, _features: &mut Self::AudioFeatures) -> Result<(), WorkerError> {
//!        println!("cycle {} ended", self.end_cycle);
//!        self.end_cycle += 1;
//!        Ok(())
//!    }
//!}
//!
//!```

use lv2_core::extension::ExtensionDescriptor;
use lv2_core::feature::*;
use lv2_core::plugin::{Plugin, PluginInstance};
use lv2_core::prelude::*;
use lv2_sys;
use std::marker::PhantomData;
use std::mem;
use std::os::raw::*; //get all common c_type
use std::ptr;

/// Host feature providing data to build a ScheduleHandler.
#[repr(transparent)]
pub struct Schedule<'a, P> {
    internal: &'a lv2_sys::LV2_Worker_Schedule,
    phantom: PhantomData<*const P>
}

unsafe impl<'a, P> UriBound for Schedule<'a, P> {
    const URI: &'static [u8] = lv2_sys::LV2_WORKER__schedule;
}

unsafe impl<'a, P> Feature for Schedule<'a, P> {
    unsafe fn from_feature_ptr(feature: *const c_void, class: ThreadingClass) -> Option<Self> {
            if class == ThreadingClass::Audio {
                (feature as *const lv2_sys::LV2_Worker_Schedule)
                    .as_ref()
                    .map(|internal| Self { internal, phantom: PhantomData::<*const P> })
            } else {
                panic!("The Worker Schedule feature is only allowed in the audio threading class");
            }
    }
}

impl<'a, P: Worker> Schedule<'a, P> {
    /// Request the host to call the worker thread.
    ///
    /// This method should be called from `run()` context to request that the host call the `work()`
    /// method in a non-realtime context with the given arguments.
    ///
    /// This function is always safe to call from `run()`, but it is not guaranteed that the worker
    /// is actually called from a different thread. In particular, when free-wheeling (e.g. for
    /// offline rendering), the worker may be executed immediately. This allows single-threaded
    /// processing with sample accuracy and avoids timing problems when `run()` is executing much
    /// faster or slower than real-time.
    ///
    /// Plugins SHOULD be written in such a way that if the worker runs immediately, and responses
    /// from the worker are delivered immediately, the effect of the work takes place immediately
    /// with sample accuracy.
    ///
    /// **Notes about the passed data:** The buffer used to pass data is managed by the host. That
    /// mean the size is unknown and may be limited. So if you need to pass huge amount of data,
    /// it's preferable to use another way, for example a sync::mpsc channel.
    pub fn schedule_work(&self, worker_data: P::WorkData) -> Result<(), WorkerError>
    where
        P::WorkData: 'static + Send,
    {
        unsafe {
            let worker_data = mem::ManuallyDrop::new(worker_data);
            let size = mem::size_of_val(&worker_data) as u32;
            let ptr = &worker_data as *const _ as *const c_void;
            let schedule_work = if let Some(schedule_work) = self.internal.schedule_work {
                schedule_work
            } else {
                return Err(WorkerError::Unknown);
            };
            match (schedule_work)(self.internal.handle, size, ptr) {
                lv2_sys::LV2_Worker_Status_LV2_WORKER_SUCCESS => Ok(()),
                lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN => Err(WorkerError::Unknown),
                lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_NO_SPACE => Err(WorkerError::NoSpace),
                _ => Err(WorkerError::Unknown),
            }
        }
    }
}

/// Handler available inside worker function to send response to `run()` context.
///
/// ResponseHandler need the current Worker trait implementor as generic parameter, because it use
/// the `ResponseData` associated type to know the datatype to send to the `worker_response`
/// method.
pub struct ResponseHandler<P: Worker> {
    /// function provided by the host to send response to `run()`
    response_function: lv2_sys::LV2_Worker_Respond_Function,
    /// Response handler provided by the host, must be passed to the host provided
    /// response_function.
    respond_handle: lv2_sys::LV2_Worker_Respond_Handle,
    phantom: PhantomData<P>,
}

impl<P: Worker> ResponseHandler<P> {
    /// Allow to give response to the `run` context.
    ///
    /// This method allow to give a response to the `run` context. After calling this method, the host will call
    /// `worker_response` and give the passed data.
    ///
    /// **Notes about the passed data:** The buffer used to pass data is managed by the host. That
    /// mean the size is unknown and may be limited. So if you need to pass huge amount of data,
    /// it's preferable to use another way, for example a sync::mpsc channel.
    pub fn respond(&self, response_data: P::ResponseData) -> Result<(), WorkerError>
    where
        P::WorkData: 'static + Send,
    {
        unsafe {
            let response_data = mem::ManuallyDrop::new(response_data);
            let size = mem::size_of_val(&response_data) as u32;
            let ptr = &response_data as *const _ as *const c_void;
            let response_function = self.response_function.ok_or(WorkerError::Unknown)?;
            match (response_function)(self.respond_handle, size, ptr) {
                lv2_sys::LV2_Worker_Status_LV2_WORKER_SUCCESS => Ok(()),
                lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN => Err(WorkerError::Unknown),
                lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_NO_SPACE => Err(WorkerError::NoSpace),
                _ => Err(WorkerError::Unknown),
            }
        }
    }
}

/// Errors potentially generated by [`Worker`](trait.Worker.html) methods
pub enum WorkerError {
    /// Unknown or general error
    Unknown,
    /// Failure due to a lack of space
    NoSpace,
}

/// The trait providing worker extension to rust-lv2 plugins.
///
/// This trait, and `struct` that implements it, provide methods called by the host to implement
/// the worker extension.
///
/// However, the host will not directly use these methods. Instead, the host use wrapping functions
/// associated with [`WorkerDescriptor`](struct.WorkerDescriptor.html) and then, you pass this
/// functions to the host with `match_extensions!` macro in `extension_data()` methods of the
/// Plugin trait.
pub trait Worker: Plugin {
    /// Type of data sent to `work` by using a schedule handler.
    type WorkData: 'static + Send;
    /// Type of data sent to `work_response` by using a response handler.
    type ResponseData: 'static + Send;
    /// The work to do in a non-real-time context,
    ///
    /// This is called by the host in a non-realtime context as requested, probably in a separate
    /// thread from `run()` and possibly with an arbitrary message to handle.
    ///
    /// A response can be sent to `run()` context using the response handler. The plugin MUST NOT make any assumptions
    /// about which thread calls this method, except that there are no real-time requirements and
    /// only one call may be executed at a time. That is, the host MAY call this method from any
    /// non-real-time thread, but MUST NOT make concurrent calls to this method from several
    /// threads.
    fn work(
        &mut self,
        response_handler: &ResponseHandler<Self>,
        data: Self::WorkData,
    ) -> Result<(), WorkerError>;

    /// Handle a response from the worker.
    ///
    /// This is called by the host in the `run()` context when a response from the worker is ready.
    fn work_response(&mut self, _data: Self::ResponseData, _features: &mut Self::AudioFeatures) -> Result<(), WorkerError> {
        Ok(())
    }

    ///Called when all responses for this cycle have been delivered.
    ///
    ///Since work_response() may be called after `run()` finished, this method provides a hook for code that
    ///must run after the cycle is completed.
    fn end_run(&mut self, _features: &mut Self::AudioFeatures) -> Result<(), WorkerError> {
        Ok(())
    }
}

///Raw wrapper of the [`Worker`](trait.Worker.html) extension.
///
/// This is a marker type that has the required external methods for the extension.
pub struct WorkerDescriptor<P: Worker> {
    plugin: PhantomData<P>,
}

unsafe impl<P: Worker> UriBound for WorkerDescriptor<P> {
    const URI: &'static [u8] = lv2_sys::LV2_WORKER__interface;
}

impl<P: Worker> WorkerDescriptor<P> {
    /// Extern unsafe version of `work` method actually called by the host
    unsafe extern "C" fn extern_work(
        handle: lv2_sys::LV2_Handle,
        response_function: lv2_sys::LV2_Worker_Respond_Function,
        respond_handle: lv2_sys::LV2_Worker_Respond_Handle,
        size: u32,
        data: *const c_void,
    ) -> lv2_sys::LV2_Worker_Status {
        //deref plugin_instance and get the plugin
        let plugin_instance =
            if let Some(plugin_instance) = (handle as *mut PluginInstance<P>).as_mut() {
                plugin_instance
            } else {
                return lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN;
            };
        let plugin = plugin_instance.instance_mut();
        //build response handler
        let response_handler = ResponseHandler {
            response_function,
            respond_handle,
            phantom: PhantomData::<P>,
        };
        //build ref to worker data from raw pointer
        let worker_data =
            ptr::read_unaligned(data as *const mem::ManuallyDrop<<P as Worker>::WorkData>);
        let worker_data = mem::ManuallyDrop::into_inner(worker_data);
        if size as usize != mem::size_of_val(&worker_data) {
            return lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN;
        }
        match plugin.work(&response_handler, worker_data) {
            Ok(()) => lv2_sys::LV2_Worker_Status_LV2_WORKER_SUCCESS,
            Err(WorkerError::Unknown) => lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN,
            Err(WorkerError::NoSpace) => lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_NO_SPACE,
        }
    }

    /// Extern unsafe version of `work_response` method actually called by the host
    unsafe extern "C" fn extern_work_response(
        handle: lv2_sys::LV2_Handle,
        size: u32,
        body: *const c_void,
    ) -> lv2_sys::LV2_Worker_Status {
        //deref plugin_instance and get the plugin
        let plugin_instance =
            if let Some(plugin_instance) = (handle as *mut PluginInstance<P>).as_mut() {
                plugin_instance
            } else {
                return lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN;
            };
        //let plugin = plugin_instance.instance_mut();
        //build ref to response data from raw pointer
        let response_data =
            ptr::read_unaligned(body as *const mem::ManuallyDrop<<P as Worker>::ResponseData>);
        let response_data = mem::ManuallyDrop::into_inner(response_data);
        if size as usize != mem::size_of_val(&response_data) {
            return lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN;
        }

        match plugin_instance.instance.work_response(response_data, &mut plugin_instance.audio_features) {
            Ok(()) => lv2_sys::LV2_Worker_Status_LV2_WORKER_SUCCESS,
            Err(WorkerError::Unknown) => lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN,
            Err(WorkerError::NoSpace) => lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_NO_SPACE,
        }
    }

    /// Extern unsafe version of `end_run` method actually called by the host
    unsafe extern "C" fn extern_end_run(handle: lv2_sys::LV2_Handle) -> lv2_sys::LV2_Worker_Status {
        if let Some(plugin_instance) = (handle as *mut PluginInstance<P>).as_mut() {
            //let plugin = plugin_instance.instance_mut();
            match plugin_instance.instance.end_run(&mut plugin_instance.audio_features) {
                Ok(()) => lv2_sys::LV2_Worker_Status_LV2_WORKER_SUCCESS,
                Err(WorkerError::Unknown) => lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN,
                Err(WorkerError::NoSpace) => lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_NO_SPACE,
            }
        } else {
            lv2_sys::LV2_Worker_Status_LV2_WORKER_ERR_UNKNOWN
        }
    }
}

// Implementing the trait that contains the interface.
impl<P: Worker> ExtensionDescriptor for WorkerDescriptor<P> {
    type ExtensionInterface = lv2_sys::LV2_Worker_Interface;

    const INTERFACE: &'static lv2_sys::LV2_Worker_Interface = &lv2_sys::LV2_Worker_Interface {
        work: Some(Self::extern_work),
        work_response: Some(Self::extern_work_response),
        end_run: Some(Self::extern_end_run),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use lv2_sys::*;
    use std::fmt;
    use std::mem;
    use std::ops;
    use std::ptr;

    // structure to test drooping issue
    struct HasDrop {
        drop_count: u32,
        drop_limit: u32,
    }

    impl HasDrop {
        fn new(val: u32) -> Self {
            Self {
                drop_count: 0,
                drop_limit: val,
            }
        }
    }

    impl ops::Drop for HasDrop {
        fn drop(&mut self) {
            if self.drop_count >= self.drop_limit {
                panic!("Dropped more than {} time", self.drop_limit);
            } else {
                self.drop_count += 1;
            }
        }
    }

    impl fmt::Display for HasDrop {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "HasDrop variable")
        }
    }
    #[derive(PortCollection)]
    struct Ports {}
    struct TestDropWorker;
    // URI identifier
    unsafe impl<'a> UriBound for TestDropWorker {
        const URI: &'static [u8] = b"not relevant\0";
    }
    impl Plugin for TestDropWorker {
        type Ports = Ports;
        type InitFeatures = ();
        type AudioFeatures = ();

        fn new(_plugin_info: &PluginInfo, _features: &mut Self::InitFeatures) -> Option<Self> {
            Some(Self {})
        }

        fn run(&mut self, _ports: &mut Ports, _features: &mut Self::InitFeatures) {}
    }

    impl Worker for TestDropWorker {
        type WorkData = HasDrop;
        type ResponseData = HasDrop;

        fn work(
            &mut self,
            _response_handler: &ResponseHandler<Self>,
            _data: HasDrop,
        ) -> Result<(), WorkerError> {
            Ok(())
        }

        fn work_response(&mut self, _data: HasDrop, _features: &mut  Self::AudioFeatures ) -> Result<(), WorkerError> {
            Ok(())
        }
    }

    extern "C" fn extern_schedule(
        _handle: LV2_Worker_Schedule_Handle,
        _size: u32,
        _data: *const c_void,
    ) -> LV2_Worker_Status {
        LV2_Worker_Status_LV2_WORKER_SUCCESS
    }

    extern "C" fn extern_respond(
        _handle: LV2_Worker_Respond_Handle,
        _size: u32,
        _data: *const c_void,
    ) -> LV2_Worker_Status {
        LV2_Worker_Status_LV2_WORKER_SUCCESS
    }

    #[test]
    fn schedule_must_not_drop() {
        let hd = HasDrop::new(0);
        let internal = lv2_sys::LV2_Worker_Schedule {
            handle :ptr::null_mut(),
            schedule_work: Some(extern_schedule),
        };
        let schedule = Schedule {
            internal: & internal,
            phantom: PhantomData::<*const TestDropWorker>,
        };
        let _ = schedule.schedule_work(hd);
    }

    #[test]
    fn respond_must_not_drop() {
        let hd = HasDrop::new(0);
        let respond = ResponseHandler {
            response_function: Some(extern_respond),
            respond_handle: ptr::null_mut(),
            phantom: PhantomData::<TestDropWorker>,
        };
        let _ = respond.respond(hd);
    }

    #[test]
    #[should_panic(expected = "Dropped")]
    fn extern_work_should_drop() {
        unsafe {
            let hd = mem::ManuallyDrop::new(HasDrop::new(0));
            let ptr_hd = &hd as *const _ as *const c_void;
            let size = mem::size_of_val(&hd) as u32;
            let mut tdw = TestDropWorker {};

            let ptr_tdw = &mut tdw as *mut _ as *mut c_void;
            //trash trick i use Plugin ptr insteas of Pluginstance ptr
            WorkerDescriptor::<TestDropWorker>::extern_work(
                ptr_tdw,
                Some(extern_respond),
                ptr::null_mut(),
                size,
                ptr_hd,
            );
        }
    }

    #[test]
    fn extern_work_should_not_drop_twice() {
        unsafe {
            let hd = mem::ManuallyDrop::new(HasDrop::new(1));
            let ptr_hd = &hd as *const _ as *const c_void;
            let size = mem::size_of_val(&hd) as u32;
            let mut tdw = TestDropWorker {};

            let ptr_tdw = &mut tdw as *mut _ as *mut c_void;
            //trash trick i use Plugin ptr insteas of Pluginstance ptr
            WorkerDescriptor::<TestDropWorker>::extern_work(
                ptr_tdw,
                Some(extern_respond),
                ptr::null_mut(),
                size,
                ptr_hd,
            );
        }
    }

    #[test]
    #[should_panic(expected = "Dropped")]
    fn extern_work_response_should_drop() {
        unsafe {
            let hd = mem::ManuallyDrop::new(HasDrop::new(0));
            let ptr_hd = &hd as *const _ as *const c_void;
            let size = mem::size_of_val(&hd) as u32;
            let mut tdw = TestDropWorker {};

            let ptr_tdw = &mut tdw as *mut _ as *mut c_void;
            //trash trick i use Plugin ptr insteas of Pluginstance ptr
            WorkerDescriptor::<TestDropWorker>::extern_work_response(ptr_tdw, size, ptr_hd);
        }
    }

    #[test]
    fn extern_work_response_should_not_drop_twice() {
        unsafe {
            let hd = mem::ManuallyDrop::new(HasDrop::new(1));
            let ptr_hd = &hd as *const _ as *const c_void;
            let size = mem::size_of_val(&hd) as u32;
            let mut tdw = TestDropWorker {};

            let ptr_tdw = &mut tdw as *mut _ as *mut c_void;
            //trash trick i use Plugin ptr insteas of Pluginstance ptr
            WorkerDescriptor::<TestDropWorker>::extern_work_response(ptr_tdw, size, ptr_hd);
        }
    }
}