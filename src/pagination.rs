use serde::Serialize;

pub const DEFAULT_PAGE_SIZE: usize = 20;
pub const MAX_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, Serialize)]
pub struct Pagination {
    pub page: usize,
    pub page_size: usize,
    pub total_items: usize,
    pub total_pages: usize,
    pub has_previous: bool,
    pub has_next: bool,
}

pub fn page<T: Clone>(
    items: &[T],
    requested_page: Option<usize>,
    requested_size: Option<usize>,
) -> Option<(Vec<T>, Pagination)> {
    let page = requested_page.unwrap_or(1);
    let page_size = requested_size.unwrap_or(DEFAULT_PAGE_SIZE);
    if page == 0 || page_size == 0 || page_size > MAX_PAGE_SIZE {
        return None;
    }
    let total_items = items.len();
    let total_pages = total_items.div_ceil(page_size).max(1);
    let start = page.saturating_sub(1).saturating_mul(page_size);
    let selected = items
        .get(start..start.saturating_add(page_size).min(total_items))
        .unwrap_or_default()
        .to_vec();
    Some((
        selected,
        Pagination {
            page,
            page_size,
            total_items,
            total_pages,
            has_previous: page > 1,
            has_next: page < total_pages,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::page;

    #[test]
    fn paginates_a_bounded_archive() {
        let values: Vec<_> = (0..73).collect();
        let (items, result) = page(&values, Some(4), Some(20)).unwrap_or_else(|| panic!("page"));
        assert_eq!(items, (60..73).collect::<Vec<_>>());
        assert_eq!(result.total_items, 73);
        assert_eq!(result.total_pages, 4);
        assert!(result.has_previous);
        assert!(!result.has_next);
    }

    #[test]
    fn rejects_unbounded_page_sizes() {
        assert!(page::<u8>(&[], Some(1), Some(101)).is_none());
        let (items, result) = page(&[0; 120], Some(1), Some(100)).unwrap_or_else(|| panic!("page"));
        assert_eq!(items.len(), 100);
        assert_eq!(result.total_pages, 2);
    }
}
