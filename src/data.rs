use boa_engine::{
    Context, JsError, JsResult, JsValue, NativeFunction, js_string, object::ObjectInitializer,
    property::Attribute,
};
use depth_tree::Tree;
use geo::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::Query;

#[derive(Debug, Default)]
pub struct Data {
    pub shapes: Arc<Mutex<Vec<Polygon>>>,
    pub depths: Arc<Mutex<Vec<usize>>>,
    pub groups: Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    pub context: Context,
}

fn get_polygons(
    shapes: &Arc<Mutex<Vec<Polygon>>>,
    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    args: &[JsValue],
) -> Vec<Polygon> {
    let shapes = shapes.lock().unwrap();
    let groups = groups.lock().unwrap();

    let mut iter = args.iter();
    match (iter.next(), iter.next(), iter.next()) {
        (Some(JsValue::Integer(index)), _, _) => vec![shapes[*index as usize].clone()],
        (
            Some(JsValue::String(name)),
            Some(JsValue::Integer(index1)),
            Some(JsValue::Integer(index2)),
        ) => vec![
            shapes[groups[&name.to_std_string_lossy()][*index1 as usize][*index2 as usize]].clone(),
        ],
        (Some(JsValue::String(name)), Some(JsValue::Integer(index)), None) => {
            if let Some(group) = groups.get(&name.to_std_string_lossy()) {
                let mut polygons = Vec::new();
                for index in &group[*index as usize] {
                    polygons.push(shapes[*index].clone());
                }
                polygons
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn get_points(polygons: &[Polygon]) -> Vec<Point> {
    polygons
        .into_iter()
        .map(|polygon| polygon.exterior().points())
        .flatten()
        .collect::<Vec<Point>>()
}

impl From<Vec<Polygon>> for Data {
    fn from(value: Vec<Polygon>) -> Self {
        let len = value.len();
        println!("Len: {}", len);
        let tree: Tree<Polygon> = Tree::from_polygon(value);
        println!("Built Tree");

        let mut shapes = Vec::with_capacity(len);
        let mut depths = Vec::with_capacity(len);

        for (depth, polygon) in tree.iter() {
            shapes.push(polygon.clone());
            depths.push(depth);
        }

        let shapes = Arc::new(Mutex::new(shapes));
        let depths = Arc::new(Mutex::new(depths));
        let groups = Arc::new(Mutex::new(
            vec![("main".into(), (0..len).map(|x| vec![x]).collect())]
                .into_iter()
                .collect::<HashMap<String, Vec<Vec<usize>>>>(),
        ));

        let mut context = Context::default();
        unsafe {
            {
                let depths = depths.clone();
                context.register_global_callable(
                    "depth".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let depths = depths.lock().unwrap();
                            match args.first() {
                                Some(JsValue::Integer(index)) => {
                                    JsResult::Ok(JsValue::new(depths[*index as usize]))
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                context.register_global_callable(
                    "area".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let shapes = shapes.lock().unwrap();
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (Some(JsValue::Integer(index)), _, _) => JsResult::Ok(
                                    JsValue::new(shapes[*index as usize].unsigned_area()),
                                ),
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => JsResult::Ok(JsValue::new(
                                    shapes[groups[&name.to_std_string_lossy()][*index1 as usize]
                                        [*index2 as usize]]
                                        .unsigned_area(),
                                )),
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index)),
                                    None,
                                ) => {
                                    if let Some(group) = groups.get(&name.to_std_string_lossy()) {
                                        let mut area = 0.0;
                                        for index in &group[*index as usize] {
                                            area += shapes[*index].unsigned_area();
                                        }

                                        JsResult::Ok(JsValue::new(area))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let groups = groups.clone();
                context.register_global_callable(
                    "group_index".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => {
                                    let name: String = name.to_std_string_lossy();
                                    if let Some(group) = groups.get(&name) {
                                        if *index1 < 0 || *index1 as usize >= group.len() {
                                            JsResult::Err(JsError::from_opaque(
                                                js_string!("Index out of bounds").into(),
                                            ))
                                        } else if *index2 < 0
                                            || *index2 as usize >= group[*index1 as usize].len()
                                        {
                                            JsResult::Err(JsError::from_opaque(
                                                js_string!("Index out of bounds").into(),
                                            ))
                                        } else {
                                            JsResult::Ok(JsValue::new(
                                                group[*index1 as usize][*index2 as usize],
                                            ))
                                        }
                                    } else {
                                        JsResult::Err(JsError::from_opaque(
                                            js_string!("Name not found in groups.").into(),
                                        ))
                                    }
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                context.register_global_callable(
                    "frame".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let shapes = shapes.lock().unwrap();
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (Some(JsValue::Integer(index)), _, _) => {
                                    if let Some(bounding_rect) =
                                        shapes[*index as usize].bounding_rect()
                                    {
                                        let object = ObjectInitializer::new(context)
                                            .property(
                                                js_string!("height"),
                                                bounding_rect.height(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("width"),
                                                bounding_rect.width(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_x"),
                                                bounding_rect.min().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_y"),
                                                bounding_rect.min().y,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_x"),
                                                bounding_rect.max().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_y"),
                                                bounding_rect.max().y,
                                                Attribute::all(),
                                            )
                                            .build();
                                        JsResult::Ok(JsValue::new(object))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => {
                                    if let Some(bounding_rect) = shapes[groups
                                        [&name.to_std_string_lossy()]
                                        [*index1 as usize]
                                        [*index2 as usize]]
                                        .bounding_rect()
                                    {
                                        let object = ObjectInitializer::new(context)
                                            .property(
                                                js_string!("height"),
                                                bounding_rect.height(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("width"),
                                                bounding_rect.width(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_x"),
                                                bounding_rect.min().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_y"),
                                                bounding_rect.min().y,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_x"),
                                                bounding_rect.max().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_y"),
                                                bounding_rect.max().y,
                                                Attribute::all(),
                                            )
                                            .build();
                                        JsResult::Ok(JsValue::new(object))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index)),
                                    None,
                                ) => {
                                    if let Some(bounding_rect) = MultiPolygon::new(
                                        groups[&name.to_std_string_lossy()][*index as usize]
                                            .iter()
                                            .map(|index| shapes[*index].clone())
                                            .collect::<Vec<Polygon>>(),
                                    )
                                    .bounding_rect()
                                    {
                                        let object = ObjectInitializer::new(context)
                                            .property(
                                                js_string!("height"),
                                                bounding_rect.height(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("width"),
                                                bounding_rect.width(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_x"),
                                                bounding_rect.min().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_y"),
                                                bounding_rect.min().y,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_x"),
                                                bounding_rect.max().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_y"),
                                                bounding_rect.max().y,
                                                Attribute::all(),
                                            )
                                            .build();
                                        JsResult::Ok(JsValue::new(object))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                context.register_global_callable(
                    "len".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let shapes = shapes.lock().unwrap();
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (Some(JsValue::Integer(index)), _, _) => JsResult::Ok(
                                    JsValue::new(shapes[*index as usize].rings().count()),
                                ),
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => JsResult::Ok(JsValue::new(
                                    shapes[groups[&name.to_std_string_lossy()][*index1 as usize]
                                        [*index2 as usize]]
                                        .rings()
                                        .count(),
                                )),
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index)),
                                    None,
                                ) => JsResult::Ok(JsValue::new(
                                    groups[&name.to_std_string_lossy()][*index as usize].len(),
                                )),
                                (Some(JsValue::String(name)), None, None) => JsResult::Ok(
                                    JsValue::new(groups[&name.to_std_string_lossy()].len()),
                                ),
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                context.register_global_callable(
                    "center".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let polygons = get_polygons(&shapes, &groups, args);
                            let points = get_points(&polygons);
                            let total = points.iter().fold(Point::new(0.0, 0.0), |mut acc, &x| {
                                acc += x;
                                acc
                            });
                            let total = total / (points.len() as f64);

                            let object = ObjectInitializer::new(context)
                                .property(js_string!("x"), total.x(), Attribute::all())
                                .property(js_string!("y"), total.y(), Attribute::all())
                                .build();
                            JsResult::Ok(JsValue::new(object))
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                context.register_global_callable(
                    "circle_metrics".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let polygons = get_polygons(&shapes, &groups, &args);
                            let points = get_points(&polygons);
                            let total = points.iter().fold(Point::new(0.0, 0.0), |mut acc, &x| {
                                acc += x;
                                acc
                            });
                            let total = total / (points.len() as f64);

                            let mut distances = Vec::new();
                            let mut total_d = 0.;
                            for point in &points {
                                use geo::{Distance, Euclidean};

                                let d = Euclidean.distance(*point, total);
                                total_d += d;
                                distances.push(d);
                            }

                            let average_d = total_d / points.len() as f64;
                            let mut variance = 0.0;
                            for d in &distances {
                                variance += (d - average_d).powi(2);
                            }

                            let object = ObjectInitializer::new(context)
                                .property(js_string!("variance"), variance, Attribute::all())
                                .property(
                                    js_string!("circle"),
                                    1.0 - variance / average_d,
                                    Attribute::all(),
                                )
                                .build();
                            JsResult::Ok(JsValue::new(object))
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                context.register_global_callable(
                    "distance".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            let mut index = 0;
                            for arg in args {
                                // skip the first one
                                if index == 0 {
                                    index += 1;
                                    continue;
                                }

                                if let JsValue::String(_) = arg {
                                    break;
                                }
                                index += 1;
                            }

                            let (first, second) = args.split_at(index);

                            let first = get_polygons(&shapes, &groups, &first);
                            let second = get_polygons(&shapes, &groups, &second);

                            let first = MultiPolygon::from(first);
                            let second = MultiPolygon::from(second);

                            use geo::{Distance, Euclidean};
                            let distance = Euclidean.distance(&first, &second);

                            JsResult::Ok(JsValue::new(distance))
                        },
                    ),
                );
            }
        }

        println!("Build Data");
        Self {
            shapes,
            depths,
            groups,
            context,
        }
    }
}

impl From<Box<std::path::Path>> for Data {
    fn from(value: Box<std::path::Path>) -> Self {
        let lines = depth_tree::import_svg(&(*value), 0.0001).unwrap();

        let polygons: Vec<Polygon> = lines
            .into_iter()
            .map(|line| Polygon::new(line, Vec::new()))
            .collect();

        polygons.into()
    }
}

impl From<(Box<std::path::Path>, f64)> for Data {
    fn from(value: (Box<std::path::Path>, f64)) -> Self {
        let lines =
            MultiLineString::new(depth_tree::import_svg(&(*value.0), value.1 as f32).unwrap());
        println!("DONE THIS");
        let mut lines = lines.simplify(value.1);
        println!("Simplified");

        let (min_x, min_y) = lines.bounding_rect().unwrap().min().x_y();
        lines.translate_mut(-min_x, -min_y);
        // lines.scale_mut(0.01);
        let lines = lines.0;

        let polygons: Vec<Polygon> = lines
            .into_iter()
            .map(|line| Polygon::new(line, Vec::new()))
            .collect();

        polygons.into()
    }
}

impl Data {
    pub fn query<T: Query>(&mut self, queries: Vec<T>) -> Result<(), String> {
        let mut i = 0;
        let n = queries.len();
        for mut query in queries {
            println!("QUERY: {} out of {}", i, n);
            i += 1;
            query.query(self)?;
        }

        Ok(())
    }
}
