use crate::*;

#[derive(Debug, Clone)]
pub struct LoopOver<T = Box<dyn Query>>
where
    T: Query,
{
    pub get_group: String,
    pub iterator_name: String,
    pub instructions: Vec<T>,
}

impl<T: Query> Query for LoopOver<T> {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let groups = {
            let data_groups = data.groups.lock().unwrap();
            if let Some(group) = data_groups.get(&self.get_group) {
                group.clone()
            } else {
                return Err(format!("Could not find '{}' in groups.", self.get_group));
            }
        };

        let iterator_name = self.iterator_name.clone();

        for group in groups.clone() {
            {
                let mut data_groups = data.groups.lock().unwrap();
                data_groups.insert(iterator_name.clone(), vec![group]);
            }

            for instruction in &mut self.instructions {
                instruction.query(data)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use geo::polygon;

    use crate::*;

    #[test]
    fn it_works() {
        let mut data = Data::from(vec![polygon! {(0.0, 0.0).into()}]);

        let instructions: Vec<Box<dyn Query>> = vec![
            Box::from(GroupBy {
                set_group: "output".into(),
                get_group: "main".into(),
                code: "true".into(),
            }),
            Box::from(Filter {
                set_group: "output".into(),
                get_group: "main".into(),
                code: "true".into(),
            }),
        ];
        let mut loopover = LoopOver {
            get_group: "main".into(),
            iterator_name: "iter".into(),
            instructions,
        };

        if let Err(err) = loopover.query(&mut data) {
            println!("Error: {}", err);
            assert!(false);
        }

        let groups = data.groups.lock().unwrap();

        let main = groups.get("main");
        assert!(main.is_some());
        let main = main.unwrap();
        assert_eq!(main, &vec![vec![0]]);

        let output = groups.get("output");
        assert!(output.is_some());
        let output = output.unwrap();
        assert_eq!(output, &vec![vec![0]]);
    }
}
