"""最小 Kubernetes 元数据发现器：Pod UID → Node → Deployment。

只读取当前命名空间的 Pods/ReplicaSets，通过专用 ServiceAccount 的只读 Role
访问 API。发送的是元数据，不伪造指标。生产版可替换为 Rust Watch 适配器，
后端的 JSON 契约和拓扑领域模型无需改变。
"""

import json
import os
import ssl
import time
import urllib.error
import urllib.request


NAMESPACE = os.environ.get("PILOT_NAMESPACE", "linux-pilot-demo")
CLUSTER_ID = os.environ.get("PILOT_CLUSTER_ID", "linux-pilot-sim")
BACKEND = os.environ.get("PILOT_BACKEND", "http://host.docker.internal:3000")
OPERATOR_TOKEN = os.environ["PILOT_OPERATOR_TOKEN"]
SA_DIR = "/var/run/secrets/kubernetes.io/serviceaccount"
KUBE_TOKEN = open(f"{SA_DIR}/token", encoding="utf-8").read().strip()
SSL_CONTEXT = ssl.create_default_context(cafile=f"{SA_DIR}/ca.crt")
KUBE_API = f"https://{os.environ['KUBERNETES_SERVICE_HOST']}:{os.environ['KUBERNETES_SERVICE_PORT']}"


def get_resource(path):
    request = urllib.request.Request(
        KUBE_API + path,
        headers={"Authorization": f"Bearer {KUBE_TOKEN}"},
    )
    with urllib.request.urlopen(request, context=SSL_CONTEXT, timeout=8) as response:
        return json.load(response)["items"]


def owner_of(object_):
    return next(
        (item for item in object_.get("metadata", {}).get("ownerReferences", []) if item.get("controller")),
        None,
    )


def collect():
    pods = get_resource(f"/api/v1/namespaces/{NAMESPACE}/pods")
    replica_sets = {
        item["metadata"]["name"]: item
        for item in get_resource(f"/apis/apps/v1/namespaces/{NAMESPACE}/replicasets")
    }
    now = int(time.time() * 1000)
    observations = []
    for pod in pods:
        metadata = pod["metadata"]
        owner = owner_of(pod)
        if owner is None:
            continue
        kind, name = owner["kind"], owner["name"]
        if kind == "ReplicaSet" and name in replica_sets:
            higher = owner_of(replica_sets[name])
            if higher:
                kind, name = higher["kind"], higher["name"]
        conditions = pod.get("status", {}).get("conditions", [])
        ready = any(item.get("type") == "Ready" and item.get("status") == "True" for item in conditions)
        node = pod.get("spec", {}).get("nodeName")
        if not node:
            continue
        observations.append(
            {
                "clusterId": CLUSTER_ID,
                "podUid": metadata["uid"],
                "podName": metadata["name"],
                "nodeId": node,
                "namespace": metadata["namespace"],
                "workloadKind": kind,
                "workloadName": name,
                "ready": ready,
                "observedMs": now,
            }
        )
    return observations


def publish(observations):
    request = urllib.request.Request(
        BACKEND + "/api/v1/topology/pods",
        data=json.dumps(observations).encode(),
        headers={"Authorization": f"Bearer {OPERATOR_TOKEN}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=8) as response:
        return json.load(response)


while True:
    try:
        observations = collect()
        result = publish(observations)
        print(f"拓扑同步：{result['accepted']} 个 Pod", flush=True)
    except (OSError, KeyError, ValueError, urllib.error.URLError) as error:
        print(f"拓扑同步失败：{error}", flush=True)
    time.sleep(15)
