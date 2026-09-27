/** Web 与脚本共享的 TopologyManifest v1alpha1 类型。运行时仍由后端严格校验。 */
export type DeploymentMode = 'kubernetes' | 'standalone'
export interface ClusterSpec { id: string; type: DeploymentMode }
export interface ScoreProfileSpec { id: string; scope: string; engine: string; scenario: string; weights: Record<string, number> }
export interface StorageCapacity { mount: string; medium: string; capacityBytes: number }
export interface NodeSpec { id: string; clusterId: string; description: string; role: string; scoreProfileRef: string; capacity: { cpuCores: number; memoryBytes: number; storage: StorageCapacity[] } }
export type ServiceSelector =
  | { type: 'kubernetes-workload'; namespace: string; kind: string; name: string }
  | { type: 'host-process'; hostId: string; comm: string }
  | { type: 'systemd-unit'; hostId: string; unit: string }
export interface ServiceSpec { id: string; clusterId: string; description: string; criticality: string; match: ServiceSelector; dependsOn: string[] }
export interface TopologyManifest {
  apiVersion: 'linux-pilot.io/v1alpha1'
  kind: 'TopologyManifest'
  metadata: { id: string; revision: number }
  spec: { clusters: ClusterSpec[]; scoreProfiles: ScoreProfileSpec[]; nodes: NodeSpec[]; services: ServiceSpec[]; systemScore: { weights: Record<string, number>; criticalServiceMaxDelta: number } }
}
export interface ClusterView { id: string; type: DeploymentMode; node_count: number; online_nodes: number; service_count: number; resource_score: number | null }
export interface NodeView { id: string; cluster_id: string; role: string; description: string; online: boolean; score: number | null; coverage: number; profile: string; scenario: string; capacity: { cpuCores: number; memoryBytes: number; storage: StorageCapacity[] } }
export interface PodView { uid: string; name: string; node_id: string; ready: boolean; resource_score: number | null; metrics: Record<string, number> }
export interface ProcessInstance { host_id: string; pid: number; start_ticks: number; comm: string; cpu_pct: number; rss_bytes: number; metrics: Record<string, number> }
export interface ServiceView { id: string; cluster_id: string; description: string; criticality: string; pod_count?: number; covered_pods?: number; instance_count?: number; resource_score: number | null; cpu_cores: number; memory_bytes: number; read_bytes_per_s: number; write_bytes_per_s: number; network_rx_bytes_per_s: number | null; network_tx_bytes_per_s: number | null; network_covered_pods?: number; pods?: PodView[]; instances?: ProcessInstance[] }
export interface ClusterSnapshot { configured: boolean; revision?: number; clusters: ClusterView[]; nodes: NodeView[]; services: ServiceView[]; infrastructure_score: number | null; overall_score: number | null; service_coverage?: number }
