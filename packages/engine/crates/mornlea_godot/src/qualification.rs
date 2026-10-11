//! Engine-process probes are compiled only in explicitly identified test artifacts.

use godot::classes::{INode, Node};
use godot::prelude::*;

use crate::bridge::MornleaClientBridge;

#[derive(GodotClass)]
#[class(base = Node)]
struct MornleaBoundaryProbe {
    base: Base<Node>,
}

#[godot_api]
impl INode for MornleaBoundaryProbe {
    fn init(base: Base<Node>) -> Self {
        Self { base }
    }
}

#[godot_api]
impl MornleaBoundaryProbe {
    #[func]
    fn panic_result(&mut self) -> VarDictionary {
        let mut bridge = self
            .base()
            .get_node_as::<MornleaClientBridge>("../ClientBridge");
        bridge.bind_mut().qualification_panic_result()
    }

    #[func]
    fn releases(&self) -> i64 {
        let bridge = self
            .base()
            .get_node_as::<MornleaClientBridge>("../ClientBridge");
        i64::try_from(bridge.bind().qualification_releases()).expect("bounded issued cores")
    }
}
