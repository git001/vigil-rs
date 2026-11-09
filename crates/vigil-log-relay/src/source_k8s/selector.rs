//! `--container`: which containers of a pod to stream.

use k8s_openapi::api::core::v1::Pod;

#[derive(Debug, Clone)]
pub(super) enum ContainerSelector {
    /// No --container flag: K8s picks the default container.
    Default,
    /// --container all: stream every container listed in pod.spec.containers.
    All,
    /// --container app,sidecar: stream these named containers, those of
    /// them the pod has.
    Named(Vec<String>),
}

impl ContainerSelector {
    pub(super) fn from_cli(s: Option<&str>) -> Self {
        match s {
            None => Self::Default,
            Some("all") => Self::All,
            Some(s) => Self::Named(
                s.split(',')
                    .map(|n| n.trim().to_owned())
                    .filter(|n| !n.is_empty())
                    .collect(),
            ),
        }
    }

    /// Returns the container keys to stream for a given pod.
    /// An empty string key means "no container= param" (K8s default).
    ///
    /// A named container the pod does not have is left out: the API server
    /// would refuse its log, and the relay would ask again on every
    /// --watch-interval. A pod with none of the names gets no stream.
    pub(super) fn keys_for_pod(&self, pod: &Pod) -> Vec<String> {
        let containers = pod.spec.as_ref().map(|spec| &spec.containers);
        match (self, containers) {
            (Self::Default, _) | (Self::All, None) => vec![String::new()],
            (Self::All, Some(c)) => c.iter().map(|c| c.name.clone()).collect(),
            (Self::Named(names), None) => names.clone(),
            (Self::Named(names), Some(c)) => names
                .iter()
                .filter(|n| c.iter().any(|c| &c.name == *n))
                .cloned()
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use k8s_openapi::api::core::v1::{Container, PodSpec};

    use super::*;

    fn pod_with(containers: &[&str]) -> Pod {
        Pod {
            spec: Some(PodSpec {
                containers: containers
                    .iter()
                    .map(|n| Container {
                        name: (*n).to_owned(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn keys_per_selector() {
        let pod = pod_with(&["app", "sidecar"]);
        assert_eq!(ContainerSelector::from_cli(None).keys_for_pod(&pod), [""]);
        assert_eq!(
            ContainerSelector::from_cli(Some("all")).keys_for_pod(&pod),
            ["app", "sidecar"]
        );
        assert_eq!(
            ContainerSelector::from_cli(Some(" sidecar , ,app")).keys_for_pod(&pod),
            ["sidecar", "app"]
        );
        // A name the pod does not have is left out; none left: no stream.
        assert_eq!(
            ContainerSelector::from_cli(Some("app,logger")).keys_for_pod(&pod),
            ["app"]
        );
        assert!(
            ContainerSelector::from_cli(Some("logger"))
                .keys_for_pod(&pod)
                .is_empty()
        );
        // Without a spec the names are taken as they are.
        assert_eq!(
            ContainerSelector::from_cli(Some("logger")).keys_for_pod(&Pod::default()),
            ["logger"]
        );
        // all without a spec: the default container.
        assert_eq!(
            ContainerSelector::from_cli(Some("all")).keys_for_pod(&Pod::default()),
            [""]
        );
    }
}
