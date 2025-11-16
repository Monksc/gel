use crate::Data;

pub trait Query {
    fn query(&mut self, data: &mut Data) -> Result<(), String>;
}

impl Query for Box<dyn Query> {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        (**self).query(data)
    }
}

impl<T> Query for Box<T>
where
    T: Query,
{
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        (**self).query(data)
    }
}
