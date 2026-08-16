#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoListFilter {
    All,
    Open,
    Completed,
}

#[cfg(test)]
mod tests {
    use super::TodoListFilter;

    #[test]
    fn todo_list_filter_is_a_copyable_domain_value() {
        let filter = TodoListFilter::Open;
        assert_eq!(filter, TodoListFilter::Open);
    }
}
