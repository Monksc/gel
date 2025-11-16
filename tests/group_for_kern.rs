use std::io::Write;

use gel::*;

fn get_kerning_settings_outside_box() -> Vec<Box<dyn Query>> {
    vec![
            Box::from(GroupBy {
                set_group: "group".into(),
                get_group: "main".into(),
                code: "depth(group_index('group', j, 0)) == depth(group_index('main', i, 0))".into(),
            }),
            Box::from(Filter {
                set_group: "outer_symbols_not_braille".into(),
                get_group: "main".into(),
                code: "depth(group_index('main', i, 0)) == 2 && !(depth(group_index('main', i, 0)) % 2 == 0 && depth(group_index('main', i, 0)) > 0 && circle_metrics(group_index('main', i, 0)).circle > 0.9 && area(group_index('main', i, 0)) >= 0.001 && area(group_index('main', i, 0)) < 0.005)".into(),
            }),
            Box::from(Filter {
                set_group: "inner_symbols".into(),
                get_group: "main".into(),
                code: "depth(group_index('main', i, 0)) > 2".into(),
            }),
            Box::from(Sort {
                set_group: "outer_symbols_not_braille_bottom_to_top".into(),
                get_group: "outer_symbols_not_braille".into(),
                compare: "frame('outer_symbols_not_braille', l).min_y < frame('outer_symbols_not_braille', r).min_y".into(),
            }),
            Box::from(Sort {
                set_group: "outer_symbols_not_braille_left_to_right".into(),
                get_group: "outer_symbols_not_braille".into(),
                compare: "frame('outer_symbols_not_braille', l).min_x < frame('outer_symbols_not_braille', r).min_x".into(),
            }),
            Box::from(GroupBy {
                set_group: "vertical_text".into(),
                get_group: "outer_symbols_not_braille_bottom_to_top".into(),
                code: "my_group_frame = frame('vertical_text', j); ".to_string() +
                    "my_main_frame = frame('outer_symbols_not_braille_bottom_to_top', i); " +
                    "(my_group_frame.max_x - my_main_frame.max_x) ** 2 < 0.1 && " +
                    "my_group_frame.width / my_main_frame.width > 0.4 && " +
                    "my_group_frame.width / my_main_frame.width < 2.1" +
                    "",
            }),
            Box::from(GroupBy {
                set_group: "horizontal_text".into(),
                get_group: "outer_symbols_not_braille_left_to_right".into(),
                code: "my_group_frame = frame('horizontal_text', j); ".to_string() +
                    "my_main_frame = frame('outer_symbols_not_braille_left_to_right', i); " +
                    "(my_group_frame.max_y - my_main_frame.max_y) ** 2 < 0.1 && " +
                    "my_group_frame.height / my_main_frame.height > 0.4 && " +
                    "my_group_frame.height / my_main_frame.height < 2.1 && " +
                    "my_group_frame.max_x + my_group_frame.height > my_main_frame.min_x" +
                    "",
            }),
            Box::from(Filter {
                set_group: "group_text".into(),
                get_group: "horizontal_text".into(),
                code: "len('horizontal_text', i) >= 3".into(),
            }),
            Box::from(Filter {
                set_group: "braille".into(),
                get_group: "main".into(),
                code: "depth(group_index('main', i, 0)) % 2 == 0 && depth(group_index('main', i, 0)) > 0 && circle_metrics(group_index('main', i, 0)).circle > 0.9 && area(group_index('main', i, 0)) >= 0.001 && area(group_index('main', i, 0)) < 0.005".into(),
            }),
            Box::from(Sort {
                set_group: "braille".into(),
                get_group: "braille".into(),
                compare: "frame('braille', l).min_x < frame('braille', r).min_x".into(),
            }),
            Box::from(GroupBy {
                set_group: "braille_group".into(),
                get_group: "braille".into(),
                code: "distance('braille_group', j, 'braille', i) < 1.0".into(),
            }),
            // Box::from(Transformation {
            //     set_group: "braille_group".into(),
            //     get_group: "braille_group".into(),
            //     transformation: [
            //         "1.1".into(), "0.0".into(), "0.0".into(),
            //         "0.0".into(), "1.1".into(), "0.0".into(),
            //     ]
            // }),
            Box::from(Filter {
                set_group: "inside_box".into(),
                get_group: "main".into(),
                code: "depth(group_index('main', i, 0)) == 1".into(),
            }),
            Box::from(Filter {
                set_group: "outsidse_box".into(),
                get_group: "main".into(),
                code: "depth(group_index('main', i, 0)) == 0".into(),
            }),
            Box::from(Kerning {
                set_group: "kerned_text".into(),
                get_group: "group_text".into(),
                set_inner_shapes: "kerned_text_inner".into(),
                get_inner_shapes: "inner_symbols".into(),
                borders_group: "inside_box".into(),
                // space: "1.0 / 2.0".into(),
                epsilon: "0.000001".into(),
                space: "0.125".into(),
                respect_space: "frame('group_text', i, j-1).max_x + frame('group_text', i).height / 2.0 < frame('group_text', i, j).min_x".into()
            })
        ]
}

fn get_kerning_settings_no_outside_box() -> Vec<Box<dyn Query>> {
    vec![
        Box::from(Filter {
            set_group: "outer_symbols_not_braille".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) == 1 && !(depth(group_index('main', i, 0)) % 2 == 1 && circle_metrics(group_index('main', i, 0)).circle > 0.9 && area(group_index('main', i, 0)) >= 0.001 && area(group_index('main', i, 0)) < 0.005)".into(),
        }),
        Box::from(Filter {
            set_group: "inner_symbols".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) > 1".into(),
        }),
        Box::from(Sort {
            set_group: "outer_symbols_not_braille_bottom_to_top".into(),
            get_group: "outer_symbols_not_braille".into(),
            compare: "frame('outer_symbols_not_braille', l).min_y < frame('outer_symbols_not_braille', r).min_y".into(),
        }),
        Box::from(Sort {
            set_group: "outer_symbols_not_braille_left_to_right".into(),
            get_group: "outer_symbols_not_braille".into(),
            compare: "frame('outer_symbols_not_braille', l).min_x < frame('outer_symbols_not_braille', r).min_x".into(),
        }),
        Box::from(GroupBy {
            set_group: "horizontal_text".into(),
            get_group: "outer_symbols_not_braille_left_to_right".into(),
            code: "my_group_frame = frame('horizontal_text', j); ".to_string() +
                "my_main_frame = frame('outer_symbols_not_braille_left_to_right', i); " +
                "(my_group_frame.max_y - my_main_frame.max_y) ** 2 < 0.1 && " +
                "my_group_frame.height / my_main_frame.height > 0.4 && " +
                "my_group_frame.height / my_main_frame.height < 2.1 && " +
                "my_group_frame.max_x + my_group_frame.height > my_main_frame.min_x" +
                "",
        }),
        Box::from(Filter {
            set_group: "group_text".into(),
            get_group: "horizontal_text".into(),
            code: "len('horizontal_text', i) >= 3".into(),
        }),
        Box::from(Filter {
            set_group: "non_text_symbols".into(),
            get_group: "horizontal_text".into(),
            code: "len('horizontal_text', i) < 3".into(),
        }),
        Box::from(Filter {
            set_group: "braille".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) % 2 == 1 && circle_metrics(group_index('main', i, 0)).circle > 0.9 && area(group_index('main', i, 0)) >= 0.001 && area(group_index('main', i, 0)) < 0.005".into(),
        }),
        Box::from(Sort {
            set_group: "braille".into(),
            get_group: "braille".into(),
            compare: "frame('braille', l).min_x < frame('braille', r).min_x".into(),
        }),
        Box::from(GroupBy {
            set_group: "braille_group".into(),
            get_group: "braille".into(),
            code: "distance('braille_group', j, 'braille', i) < 1.0".into(),
        }),
        // Box::from(Transformation {
        //     set_group: "braille_group".into(),
        //     get_group: "braille_group".into(),
        //     transformation: [
        //         "1.1".into(), "0.0".into(), "0.0".into(),
        //         "0.0".into(), "1.1".into(), "0.0".into(),
        //     ]
        // }),
        Box::from(Filter {
            set_group: "inside_box".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) == 0".into(),
        }),
        Box::from(Kerning {
            set_group: "kerned_text".into(),
            get_group: "group_text".into(),
            set_inner_shapes: "kerned_text_inner".into(),
            get_inner_shapes: "inner_symbols".into(),
            borders_group: "inside_box".into(),
            // space: "1.0 / 2.0".into(),
            epsilon: "0.000001".into(),
            space: "0.125".into(),
            respect_space: "frame('group_text', i, j-1).max_x + frame('group_text', i).height / 3.0 < frame('group_text', i, j).min_x".into()
        })
    ]
}

#[test]
fn test_kern() {
    let queries: Vec<Box<dyn Query>> = vec![
        Box::from(GroupBy {
            set_group: "group".into(),
            get_group: "main".into(),
            code: "depth(group_index('group', j, 0)) == depth(group_index('main', i, 0))".into(),
        }),
        Box::from(GroupBy {
            set_group: "group2".into(),
            get_group: "main".into(),
            code: "area('group2', j) + area('main', i) < 5.0".into(),
        }),
        Box::from(Filter {
            set_group: "outer_symbols_not_braille".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) == 2 && !(depth(group_index('main', i, 0)) % 2 == 0 && depth(group_index('main', i, 0)) > 0 && circle_metrics(group_index('main', i, 0)).circle > 0.9 && area(group_index('main', i, 0)) >= 0.001 && area(group_index('main', i, 0)) < 0.005)".into(),
        }),
        Box::from(GroupBy {
            set_group: "group3".into(),
            get_group: "outer_symbols_not_braille".into(),
            code: "my_group_frame = frame('group3', j); ".to_string() +
                "my_main_frame = frame('outer_symbols_not_braille', i); " +
                "(my_group_frame.max_x - my_main_frame.max_x) ** 2 < 0.1 && " +
                "my_group_frame.width / my_main_frame.width > 0.4 && " +
                "my_group_frame.width / my_main_frame.width < 2.1 " +
                "",
        }),
        Box::from(Filter {
            set_group: "group_text".into(),
            get_group: "group3".into(),
            code: "len('group3', i) >= 3".into(),
        }),
        Box::from(Filter {
            set_group: "braille".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) % 2 == 0 && depth(group_index('main', i, 0)) > 0 && circle_metrics(group_index('main', i, 0)).circle > 0.9 && area(group_index('main', i, 0)) >= 0.001 && area(group_index('main', i, 0)) < 0.005".into(),
        }),
        Box::from(Transformation {
            set_group: "big_braille".into(),
            get_group: "braille".into(),
            transformation: [
                "1.0".into(), "0.0".into(), "0.0".into(),
                "0.0".into(), "1.0".into(), "0.0".into(),
            ]
        }),
        Box::from(Filter {
            set_group: "inside_box".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) == 1".into(),
        }),
        Box::from(Filter {
            set_group: "inside_box".into(),
            get_group: "main".into(),
            code: "depth(group_index('main', i, 0)) == 1".into(),
        }),
        Box::from(Kerning {
            set_group: "kerned_text".into(),
            get_group: "group_text".into(),
            set_inner_shapes: "kerned_text_inner".into(),
            get_inner_shapes: "inner_symbols".into(),
            borders_group: "inside_box".into(),
            // space: "1.0 / 2.0".into(),
            epsilon: "0.0001".into(),
            space: "0.9".into(),
            respect_space: "frame('group_text', i, j-1).max_x + frame('group_text', i).height / 2.0 < frame('group_text', i, j).min_x".into()
        })
    ];

    let path: Box<std::path::Path> = Box::from(std::path::Path::new("./testsvg/test2.svg"));
    let mut data: Data = path.into();

    let result = data.query(queries);
    println!("Result: {:?}", result);

    let groups = data.groups.lock().unwrap();
    let shapes = data.shapes.lock().unwrap();

    for (name, data) in groups.clone() {
        if name == "braille" {
            assert_eq!(data.len(), 32);
        }
        println!("{}: {:?}", name, data);
    }

    for key in [
        "group_text",
        "braille",
        "big_braille",
        "inside_box",
        "kerned_text",
    ] {
        println!("({})", key);
        if let Some(texts) = groups.get(key) {
            for text in texts {
                println!("(New Group)");
                for letter in text {
                    let polygon = &shapes[*letter as usize];
                    // println!("(New Shape)");
                    for point in polygon.exterior().points() {
                        println!("X{:.4} Y{:.4}", point.x(), point.y());
                    }
                }
            }
        }
    }
    println!("(END)");
}

#[test]
fn kern_file() {
    let queries = get_kerning_settings_no_outside_box();

    let path: Box<std::path::Path> = Box::from(std::path::Path::new("./testsvg/test1.svg"));
    let mut data: Data = (path, 0.0001).into();

    println!("Start the Queries");
    let result = data.query(queries);
    println!("Result: {:?}", result);

    let groups = data.groups.lock().unwrap();
    let shapes = data.shapes.lock().unwrap();

    let keys = [
        // "group_text",
        "braille_group",
        "inside_box",
        "outsidse_box",
        "kerned_text",
        "kerned_text_inner",
        "non_text_symbols",
    ];

    let mut border_polygon = Vec::new();
    for key in keys {
        if let Some(texts) = groups.get(key) {
            for text in texts {
                for letter in text {
                    let polygon = &shapes[*letter as usize];
                    border_polygon.push(polygon.clone());
                }
            }
        }
    }

    // let border_polygon = MultiPolygon::new(border_polygon).bounding_rect().unwrap();

    /*
    println!("(OG Shapes)");
    for i in 0..shapes.len() {
        let mut points = shapes[i].exterior().points();
        if let Some(first) = points.next() {
            println!("G00 X{:.4} Y{:.4}", first.0.x, first.0.y);
        }

        for point in points {
            println!("G01 X{:.4} Y{:.4}", point.0.x, point.0.y);
        }
    }
    */

    let mut svg_polygons = Vec::new();
    for key in keys {
        println!("({})", key);
        if let Some(texts) = groups.get(key) {
            for text in texts {
                println!("(New Group)");
                for letter in text {
                    let polygon = shapes[*letter as usize].clone();

                    let mut points = polygon.exterior().points();
                    if let Some(first) = points.next() {
                        println!("G00 X{:.4} Y{:.4}", first.0.x, first.0.y);
                    }

                    for point in points {
                        println!("G01 X{:.4} Y{:.4}", point.0.x, point.0.y);
                    }

                    svg_polygons.push(polygon)
                }
            }
        }
    }

    println!("(group_text)");
    if let Some(texts) = groups.get("group_text") {
        for text in texts {
            println!("(New Group)");
            for letter in text {
                let polygon = shapes[*letter as usize].clone();

                let mut points = polygon.exterior().points();
                if let Some(first) = points.next() {
                    println!("G00 X{:.4} Y{:.4}", first.0.x, first.0.y);
                }

                for point in points {
                    println!("G00 X{:.4} Y{:.4}", point.0.x, point.0.y);
                }
            }
        }
    }

    let mut file = std::fs::File::create("./output/finished_kerned.svg").unwrap();

    file.write_all(polygons_to_svg(&svg_polygons).as_bytes())
        .unwrap();

    println!("(END)");
}
