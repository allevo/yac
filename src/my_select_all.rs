//! An unbounded set of streams

use core::fmt::{self, Debug};
use core::iter::FromIterator;
use core::pin::Pin;
use std::task::{Context, Poll};

use futures::stream::{futures_unordered, FusedStream, FuturesUnordered, StreamFuture};
use futures::{ready, Stream, StreamExt};
use pin_project_lite::pin_project;

use crate::models::DeviceId;

pub trait GetId {
    fn get_id(&self) -> &DeviceId;
}

pin_project! {
    /// An unbounded set of streams
    ///
    /// This "combinator" provides the ability to maintain a set of streams
    /// and drive them all to completion.
    ///
    /// Streams are pushed into this set and their realized values are
    /// yielded as they become ready. Streams will only be polled when they
    /// generate notifications. This allows to coordinate a large number of streams.
    ///
    /// Note that you can create a ready-made `SelectAll` via the
    /// `select_all` function in the `stream` module, or you can start with an
    /// empty set with the `SelectAll::new` constructor.
    #[must_use = "streams do nothing unless polled"]
    pub struct MySelectAll<St: GetId> {
        #[pin]
        inner: FuturesUnordered<StreamFuture<St>>,
    }
}

impl<St: Stream + Unpin + GetId> Debug for MySelectAll<St> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ids: Vec<&DeviceId> = self
            .inner
            .iter()
            .filter_map(StreamFuture::get_ref)
            .map(GetId::get_id)
            .collect();
        write!(f, "MySelectAll {{ {:?} }}", ids)
    }
}

impl<St: Stream + Unpin + GetId> MySelectAll<St> {
    /// Constructs a new, empty `SelectAll`
    ///
    /// The returned `SelectAll` does not contain any streams and, in this
    /// state, `SelectAll::poll` will return `Poll::Ready(None)`.
    pub fn new() -> Self {
        Self {
            inner: FuturesUnordered::new(),
        }
    }

    /*

    /// Returns the number of streams contained in the set.
    ///
    /// This represents the total number of in-flight streams.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the set contains no streams
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clears the set, removing all streams.
    pub fn clear(&mut self) {
        self.inner.clear()
    }
    */

    /// Push a stream into the set.
    ///
    /// This function submits the given stream to the set for managing. This
    /// function will not call `poll` on the submitted stream. The caller must
    /// ensure that `SelectAll::poll` is called in order to receive task
    /// notifications.
    pub fn push(&mut self, stream: St) {
        self.inner.push(stream.into_future());
    }

    // TODO: do this better
    pub fn remove_by_device_id(&mut self, device_id: &DeviceId) {
        let new = FuturesUnordered::new();
        let old_new = std::mem::replace(&mut self.inner, new);

        for n in old_new {
            match n.into_inner() {
                None => continue,
                Some(s) => {
                    if s.get_id() == device_id {
                        continue;
                    }
                    self.push(s);
                }
            }
        }
    }
}

impl<St: Stream + Unpin + GetId> Default for MySelectAll<St> {
    fn default() -> Self {
        Self::new()
    }
}

impl<St: Stream + Unpin + GetId> Stream for MySelectAll<St> {
    type Item = St::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.inner.poll_next_unpin(cx)) {
                Some((Some(item), remaining)) => {
                    self.push(remaining);
                    return Poll::Ready(Some(item));
                }
                Some((None, _)) => {
                    // `FuturesUnordered` thinks it isn't terminated
                    // because it yielded a Some.
                    // We do not return, but poll `FuturesUnordered`
                    // in the next loop iteration.
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

impl<St: Stream + Unpin + GetId> FusedStream for MySelectAll<St> {
    fn is_terminated(&self) -> bool {
        self.inner.is_terminated()
    }
}

/// Convert a list of streams into a `Stream` of results from the streams.
///
/// This essentially takes a list of streams (e.g. a vector, an iterator, etc.)
/// and bundles them together into a single stream.
/// The stream will yield items as they become available on the underlying
/// streams internally, in the order they become available.
///
/// Note that the returned set can also be used to dynamically push more
/// streams into the set as they become available.
///
/// This function is only available when the `std` or `alloc` feature of this
/// library is activated, and it is activated by default.
pub fn my_select_all<I>(streams: I) -> MySelectAll<I::Item>
where
    I: IntoIterator,
    I::Item: Stream + Unpin + GetId,
{
    let mut set = MySelectAll::new();

    for stream in streams {
        set.push(stream);
    }

    // assert_stream::<<I::Item as Stream>::Item, _>(set)
    set
}

impl<St: Stream + Unpin + GetId> FromIterator<St> for MySelectAll<St> {
    fn from_iter<T: IntoIterator<Item = St>>(iter: T) -> Self {
        my_select_all(iter)
    }
}

impl<St: Stream + Unpin + GetId> Extend<St> for MySelectAll<St> {
    fn extend<T: IntoIterator<Item = St>>(&mut self, iter: T) {
        for st in iter {
            self.push(st)
        }
    }
}

impl<St: Stream + Unpin + GetId> IntoIterator for MySelectAll<St> {
    type Item = St;
    type IntoIter = IntoIter<St>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter(self.inner.into_iter())
    }
}

/*
impl<'a, St: Stream + Unpin> IntoIterator for &'a SelectAll<St> {
    type Item = &'a St;
    type IntoIter = Iter<'a, St>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, St: Stream + Unpin> IntoIterator for &'a mut SelectAll<St> {
    type Item = &'a mut St;
    type IntoIter = IterMut<'a, St>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
*/

/// Immutable iterator over all streams in the unordered set.
#[derive(Debug)]
pub struct Iter<'a, St: Unpin>(futures_unordered::Iter<'a, StreamFuture<St>>);

/// Mutable iterator over all streams in the unordered set.
#[derive(Debug)]
pub struct IterMut<'a, St: Unpin>(futures_unordered::IterMut<'a, StreamFuture<St>>);

/// Owned iterator over all streams in the unordered set.
#[derive(Debug)]
pub struct IntoIter<St: Unpin>(futures_unordered::IntoIter<StreamFuture<St>>);

impl<'a, St: Stream + Unpin + GetId> Iterator for Iter<'a, St> {
    type Item = &'a St;

    fn next(&mut self) -> Option<Self::Item> {
        let st = self.0.next()?;
        let next = st.get_ref();
        // This should always be true because FuturesUnordered removes completed futures.
        debug_assert!(next.is_some());
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<St: Stream + Unpin + GetId> ExactSizeIterator for Iter<'_, St> {}

impl<'a, St: Stream + Unpin> Iterator for IterMut<'a, St> {
    type Item = &'a mut St;

    fn next(&mut self) -> Option<Self::Item> {
        let st = self.0.next()?;
        let next = st.get_mut();
        // This should always be true because FuturesUnordered removes completed futures.
        debug_assert!(next.is_some());
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<St: Stream + Unpin + GetId> ExactSizeIterator for IterMut<'_, St> {}

impl<St: Stream + Unpin + GetId> Iterator for IntoIter<St> {
    type Item = St;

    fn next(&mut self) -> Option<Self::Item> {
        let st = self.0.next()?;
        let next = st.into_inner();
        // This should always be true because FuturesUnordered removes completed futures.
        debug_assert!(next.is_some());
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<St: Stream + Unpin + GetId> ExactSizeIterator for IntoIter<St> {}
