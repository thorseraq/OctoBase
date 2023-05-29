use std::collections::HashMap;
use yrs::{
    ArrayRef, Doc, Map, MapRef, TextRef, Transact, XmlElementRef, XmlFragment, XmlFragmentRef,
    XmlTextRef,
};

#[derive(Hash, PartialEq, Eq)]
pub enum MapOpType {
    Insert,
    Remove,
    Clear,
    InsertNestType,
}

#[derive(Default)]
pub struct MapOpParams {
    key: Option<String>,
    value: Option<String>,
}

pub enum YrsNestType {
    Array(ArrayRef),
    Map(MapRef),
    Text(TextRef),
    XMLElement(XmlElementRef),
    XMLFragment(XmlFragmentRef),
    XMLText(XmlTextRef),
}

pub struct YrsMapOps {
    ops: HashMap<MapOpType, Box<dyn Fn(&yrs::Doc, &MapRef, MapOpParams) -> Option<YrsNestType>>>,
}

impl YrsMapOps {
    fn new() -> Self {
        let mut ops: HashMap<
            MapOpType,
            Box<dyn Fn(&yrs::Doc, &MapRef, MapOpParams) -> Option<YrsNestType>>,
        > = HashMap::new();

        let insert_op = |doc: &yrs::Doc, map: &MapRef, params: MapOpParams| {
            let mut trx = doc.transact_mut();
            map.insert(&mut trx, params.key.unwrap(), params.value.unwrap())
                .unwrap();

            None
        };

        let remove_op = |doc: &yrs::Doc, map: &MapRef, params: MapOpParams| {
            let mut trx = doc.transact_mut();
            map.remove(&mut trx, params.key.unwrap().as_str());

            None
        };

        let clear_op = |doc: &yrs::Doc, map: &MapRef, params: MapOpParams| {
            let mut trx = doc.transact_mut();
            map.clear(&mut trx);

            None
        };

        ops.insert(MapOpType::Insert, Box::new(insert_op));
        ops.insert(MapOpType::Remove, Box::new(remove_op));
        ops.insert(MapOpType::Clear, Box::new(clear_op));

        Self { ops }
    }
}

pub enum OpType {
    YrsMap(MapOpType),
}
