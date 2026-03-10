/// Real-time shortest-path routing over the latency mesh.
/// Dijkstra on live latency matrix — picks fastest path, re-routes on degradation.
use super::latency_matrix::LatencyMatrix;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Shortest-path mesh router using live latency data.
/// Runs Dijkstra on the latency matrix to find the fastest route.
pub struct MeshRouter {
    local_node: String,
    matrix: Arc<LatencyMatrix>,
}

/// A computed route to a destination node.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub next_hop: String,
    pub total_rtt: Duration,
    pub path: Vec<String>,
}

#[derive(Eq, PartialEq)]
struct DijkstraState {
    cost_us: u64,
    node: String,
}

impl Ord for DijkstraState {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost_us.cmp(&self.cost_us) // min-heap
    }
}

impl PartialOrd for DijkstraState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl MeshRouter {
    pub fn new(local_node: String, matrix: Arc<LatencyMatrix>) -> Self {
        Self { local_node, matrix }
    }

    /// Find the next hop to reach `destination` via shortest path
    pub fn next_hop(&self, destination: &str) -> Option<RouteEntry> {
        if destination == self.local_node {
            return None;
        }
        let path = self.dijkstra(destination)?;
        if path.len() < 2 {
            return None;
        }
        let total_rtt = self.path_cost(&path);
        Some(RouteEntry {
            next_hop: path[1].clone(),
            total_rtt,
            path,
        })
    }

    /// Get the top-N diverse paths to a destination (for multi-path sending)
    pub fn top_paths(&self, destination: &str, n: usize) -> Vec<RouteEntry> {
        if destination == self.local_node {
            return vec![];
        }
        let edges = self.matrix.all_edges();
        let mut neighbors: HashSet<String> = HashSet::new();
        for (from, _to, _) in &edges {
            if from == &self.local_node {
                neighbors.insert(_to.clone());
            }
        }

        let mut routes: Vec<RouteEntry> = neighbors
            .iter()
            .filter_map(|neighbor| self.dijkstra_via(neighbor, destination))
            .collect();

        routes.sort_by_key(|r| r.total_rtt);
        routes.truncate(n);
        routes
    }

    /// Run Dijkstra from local_node to destination
    fn dijkstra(&self, destination: &str) -> Option<Vec<String>> {
        let edges = self.matrix.all_edges();
        let nodes = self.matrix.nodes();

        let mut dist: HashMap<String, u64> = HashMap::new();
        let mut prev: HashMap<String, String> = HashMap::new();
        let mut heap = BinaryHeap::new();

        for node in &nodes {
            dist.insert(node.clone(), u64::MAX);
        }
        dist.insert(self.local_node.clone(), 0);
        heap.push(DijkstraState {
            cost_us: 0,
            node: self.local_node.clone(),
        });

        while let Some(DijkstraState { cost_us, node }) = heap.pop() {
            if node == destination {
                break;
            }
            if cost_us > *dist.get(&node).unwrap_or(&u64::MAX) {
                continue;
            }
            for (from, to, rtt) in &edges {
                if from != &node {
                    continue;
                }
                let new_cost = cost_us.saturating_add(rtt.as_micros() as u64);
                if new_cost < *dist.get(to.as_str()).unwrap_or(&u64::MAX) {
                    dist.insert(to.clone(), new_cost);
                    prev.insert(to.clone(), node.clone());
                    heap.push(DijkstraState {
                        cost_us: new_cost,
                        node: to.clone(),
                    });
                }
            }
        }

        // Reconstruct path
        let mut path = vec![destination.to_string()];
        let mut current = destination.to_string();
        while current != self.local_node {
            match prev.get(&current) {
                Some(p) => {
                    path.push(p.clone());
                    current = p.clone();
                }
                None => return None,
            }
        }
        path.reverse();
        Some(path)
    }

    /// Dijkstra forcing first hop through `via`
    fn dijkstra_via(&self, via: &str, destination: &str) -> Option<RouteEntry> {
        let first_hop_cost = self.matrix.get_rtt(&self.local_node, via)?;
        let edges = self.matrix.all_edges();
        let nodes = self.matrix.nodes();

        let mut dist: HashMap<String, u64> = HashMap::new();
        let mut prev: HashMap<String, String> = HashMap::new();
        let mut heap = BinaryHeap::new();

        let initial_cost = first_hop_cost.as_micros() as u64;
        for node in &nodes {
            dist.insert(node.clone(), u64::MAX);
        }
        dist.insert(via.to_string(), initial_cost);
        heap.push(DijkstraState {
            cost_us: initial_cost,
            node: via.to_string(),
        });

        while let Some(DijkstraState { cost_us, node }) = heap.pop() {
            if node == destination {
                break;
            }
            if cost_us > *dist.get(&node).unwrap_or(&u64::MAX) {
                continue;
            }
            for (from, to, rtt) in &edges {
                if from != &node {
                    continue;
                }
                let new_cost = cost_us.saturating_add(rtt.as_micros() as u64);
                if new_cost < *dist.get(to.as_str()).unwrap_or(&u64::MAX) {
                    dist.insert(to.clone(), new_cost);
                    prev.insert(to.clone(), node.clone());
                    heap.push(DijkstraState {
                        cost_us: new_cost,
                        node: to.clone(),
                    });
                }
            }
        }

        if !prev.contains_key(destination) && via != destination {
            return None;
        }

        let mut path = vec![destination.to_string()];
        let mut current = destination.to_string();
        while current != *via {
            match prev.get(&current) {
                Some(p) => {
                    path.push(p.clone());
                    current = p.clone();
                }
                None => return None,
            }
        }
        path.push(self.local_node.clone());
        path.reverse();

        let total_rtt = Duration::from_micros(*dist.get(destination).unwrap_or(&u64::MAX));
        Some(RouteEntry {
            next_hop: via.to_string(),
            total_rtt,
            path,
        })
    }

    fn path_cost(&self, path: &[String]) -> Duration {
        let mut total = Duration::ZERO;
        for w in path.windows(2) {
            if let Some(rtt) = self.matrix.get_rtt(&w[0], &w[1]) {
                total += rtt;
            }
        }
        total
    }

    pub fn local_node(&self) -> &str {
        &self.local_node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_route() {
        let matrix = Arc::new(LatencyMatrix::new());
        matrix.update("syd", "sgp", Duration::from_millis(50));
        matrix.update("sgp", "syd", Duration::from_millis(50));

        let router = MeshRouter::new("syd".to_string(), matrix);
        let route = router.next_hop("sgp").unwrap();
        assert_eq!(route.next_hop, "sgp");
        assert_eq!(route.path, vec!["syd", "sgp"]);
    }

    #[test]
    fn multi_hop_shortest_path() {
        let matrix = Arc::new(LatencyMatrix::new());
        // syd → sgp: 50ms, sgp → lon: 80ms, syd → lon: 200ms
        matrix.update("syd", "sgp", Duration::from_millis(50));
        matrix.update("sgp", "lon", Duration::from_millis(80));
        matrix.update("syd", "lon", Duration::from_millis(200));

        let router = MeshRouter::new("syd".to_string(), matrix);
        let route = router.next_hop("lon").unwrap();
        // Should prefer syd → sgp → lon (130ms) over syd → lon (200ms)
        assert_eq!(route.next_hop, "sgp");
        assert_eq!(route.path, vec!["syd", "sgp", "lon"]);
    }

    #[test]
    fn no_route() {
        let matrix = Arc::new(LatencyMatrix::new());
        matrix.update("syd", "sgp", Duration::from_millis(50));

        let router = MeshRouter::new("syd".to_string(), matrix);
        assert!(router.next_hop("lon").is_none());
    }

    #[test]
    fn self_route_is_none() {
        let matrix = Arc::new(LatencyMatrix::new());
        let router = MeshRouter::new("syd".to_string(), matrix);
        assert!(router.next_hop("syd").is_none());
    }
}
