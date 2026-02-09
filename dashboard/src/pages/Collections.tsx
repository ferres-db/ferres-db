import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { useCollections, useCreateCollection, useDeleteCollection } from '@/hooks/useCollections';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Modal } from '@/components/ui/Modal';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/Table';
import { Badge } from '@/components/ui/Badge';
import { Skeleton } from '@/components/ui/Skeleton';
import { Plus, Trash2 } from 'lucide-react';
import type { QuantizationConfig, TieredStorageConfig } from '@/types';

export const Collections = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  const canEdit = role === 'admin' || role === 'editor';
  const { data: collections, isLoading } = useCollections();
  const createCollection = useCreateCollection();
  const deleteCollection = useDeleteCollection();
  const [isCreateModalOpen, setIsCreateModalOpen] = useState(false);
  const [newCollectionName, setNewCollectionName] = useState('');
  const [newVectorSize, setNewVectorSize] = useState('128');
  const [newDistanceMetric, setNewDistanceMetric] = useState('cosine');

  // SQ8 Quantization
  const [enableQuantization, setEnableQuantization] = useState(false);
  const [alwaysRam, setAlwaysRam] = useState(false);
  const [quantile, setQuantile] = useState('0.99');

  // BM25
  const [enableBm25, setEnableBm25] = useState(false);
  const [bm25TextField, setBm25TextField] = useState('text');

  // Tiered Storage
  const [enableTiered, setEnableTiered] = useState(false);
  const [hotThreshold, setHotThreshold] = useState('24');
  const [warmThreshold, setWarmThreshold] = useState('168');
  const [compactionInterval, setCompactionInterval] = useState('3600');

  const handleCreate = async () => {
    if (!newCollectionName || !newVectorSize) return;

    let quantization: QuantizationConfig | undefined;
    if (enableQuantization) {
      quantization = {
        type: 'scalar',
        dtype: 'int8',
        always_ram: alwaysRam,
        quantile: parseFloat(quantile) || 0.99,
      };
    }

    let tiered_storage: TieredStorageConfig | undefined;
    if (enableTiered) {
      tiered_storage = {
        enabled: true,
        hot_threshold_hours: parseInt(hotThreshold) || 24,
        warm_threshold_hours: parseInt(warmThreshold) || 168,
        compaction_interval_secs: parseInt(compactionInterval) || 3600,
      };
    }

    await createCollection.mutateAsync({
      name: newCollectionName,
      vectorSize: parseInt(newVectorSize),
      distanceMetric: newDistanceMetric,
      quantization,
      enable_bm25: enableBm25 || undefined,
      bm25_text_field: enableBm25 ? bm25TextField : undefined,
      tiered_storage,
    });
    setIsCreateModalOpen(false);
    setNewCollectionName('');
    setNewVectorSize('128');
    setEnableQuantization(false);
    setAlwaysRam(false);
    setQuantile('0.99');
    setEnableBm25(false);
    setBm25TextField('text');
    setEnableTiered(false);
    setHotThreshold('24');
    setWarmThreshold('168');
    setCompactionInterval('3600');
  };

  const handleDelete = async (name: string) => {
    if (confirm(`Are you sure you want to delete collection "${name}"?`)) {
      await deleteCollection.mutateAsync(name);
    }
  };

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-gray-50">Collections</h1>
          <p className="mt-1 text-sm text-gray-400">Manage your vector collections</p>
        </div>
        {canEdit && (
          <Button onClick={() => setIsCreateModalOpen(true)} size="md">
            <Plus className="h-4 w-4 mr-2" />
            New Collection
          </Button>
        )}
      </div>

      <Card>
        <CardHeader>
          <CardTitle>All Collections</CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="space-y-2">
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Vector Size</TableHead>
                  <TableHead>Distance Metric</TableHead>
                  <TableHead>Points</TableHead>
                  <TableHead>Features</TableHead>
                  <TableHead>Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {Array.isArray(collections) && collections.map((collection) => (
                  <TableRow 
                    key={collection.name}
                    className="cursor-pointer hover:bg-bg-tertiary"
                    onClick={() => navigate(`/collections/${collection.name}/points`)}
                  >
                    <TableCell className="font-medium">{collection.name}</TableCell>
                    <TableCell>{collection.vector_size ?? collection.dimension}</TableCell>
                    <TableCell>
                      <Badge variant="default">{collection.distance_metric ?? collection.distance ?? 'N/A'}</Badge>
                    </TableCell>
                    <TableCell>{collection.point_count ?? collection.num_points ?? 0}</TableCell>
                    <TableCell>
                      <div className="flex gap-1 flex-wrap">
                        {(collection as any).quantization && (collection as any).quantization !== 'None' && (
                          <Badge variant="warning" className="text-[10px]">SQ8</Badge>
                        )}
                        {(collection as any).bm25_enabled && (
                          <Badge variant="success" className="text-[10px]">BM25</Badge>
                        )}
                      </div>
                    </TableCell>
                    <TableCell>
                        {canEdit && (
                          <Button
                            variant="danger"
                            size="sm"
                            onClick={(e) => {
                              e.stopPropagation();
                              handleDelete(collection.name);
                            }}
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        )}
                      </TableCell>
                  </TableRow>
                ))}
                {(!collections || !Array.isArray(collections) || collections.length === 0) && (
                  <TableRow>
                    <TableCell colSpan={6} className="py-12 text-center text-sm text-gray-500">
                      No collections found
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      <Modal
        isOpen={isCreateModalOpen}
        onClose={() => setIsCreateModalOpen(false)}
        title="Create New Collection"
      >
        <div className="space-y-4 max-h-[70vh] overflow-y-auto">
          <div>
            <label className="block text-sm font-medium mb-1">Collection Name</label>
            <Input
              value={newCollectionName}
              onChange={(e) => setNewCollectionName(e.target.value)}
              placeholder="my-collection"
            />
          </div>
          <div>
            <label className="block text-sm font-medium mb-1">Vector Size</label>
            <Input
              type="number"
              value={newVectorSize}
              onChange={(e) => setNewVectorSize(e.target.value)}
              placeholder="128"
            />
          </div>
          <div>
            <label className="block text-sm font-medium mb-1">Distance Metric</label>
            <select
              className="flex h-9 w-full rounded-lg border border-white/[0.08] bg-white/[0.04] px-3 py-2 text-sm text-gray-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-orange-500/60 [&>option]:bg-bg-secondary [&>option]:text-gray-50"
              value={newDistanceMetric}
              onChange={(e) => setNewDistanceMetric(e.target.value)}
            >
              <option value="cosine">Cosine</option>
              <option value="euclidean">Euclidean</option>
              <option value="dot">Dot Product</option>
            </select>
          </div>

          {/* ─── SQ8 Quantization ─────────────────────────────── */}
          <div className="border-t border-white/[0.06] pt-4">
            <label className="mb-3 flex cursor-pointer items-center gap-2 text-sm font-medium">
              <input
                type="checkbox"
                checked={enableQuantization}
                onChange={(e) => setEnableQuantization(e.target.checked)}
                className="rounded border-white/[0.2] bg-white/[0.04] text-orange-500 focus:ring-orange-500"
              />
              Enable Scalar Quantization (SQ8)
            </label>
            {enableQuantization && (
              <div className="ml-6 space-y-3 rounded-lg border border-white/[0.06] bg-white/[0.03] p-3">
                <p className="text-xs text-gray-400">
                  Compresses f32 vectors to u8 (~4x memory savings) with minimal recall loss.
                </p>
                <label className="flex items-center gap-2 cursor-pointer text-sm text-gray-300">
                  <input
                    type="checkbox"
                    checked={alwaysRam}
                    onChange={(e) => setAlwaysRam(e.target.checked)}
                    className="rounded border-white/[0.2] bg-white/[0.04] text-orange-500 focus:ring-orange-500"
                  />
                  Always keep original vectors in RAM (re-rank with f32)
                </label>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">
                    Quantile ({quantile})
                  </label>
                  <input
                    type="range"
                    min="0.9"
                    max="1.0"
                    step="0.01"
                    value={quantile}
                    onChange={(e) => setQuantile(e.target.value)}
                    className="w-full accent-orange-500"
                  />
                  <div className="flex justify-between text-[10px] text-gray-500">
                    <span>0.90</span>
                    <span>1.00</span>
                  </div>
                </div>
              </div>
            )}
          </div>

          {/* ─── BM25 ─────────────────────────────────────────── */}
          <div className="border-t border-white/[0.06] pt-4">
            <label className="mb-3 flex cursor-pointer items-center gap-2 text-sm font-medium">
              <input
                type="checkbox"
                checked={enableBm25}
                onChange={(e) => setEnableBm25(e.target.checked)}
                className="rounded border-white/[0.2] bg-white/[0.04] text-orange-500 focus:ring-orange-500"
              />
              Enable BM25 Full-Text Search
            </label>
            {enableBm25 && (
              <div className="ml-6 space-y-3 rounded-lg border border-white/[0.06] bg-white/[0.03] p-3">
                <p className="text-xs text-gray-400">
                  Enables hybrid search (vector + keyword) via BM25 ranking.
                </p>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Text Metadata Field</label>
                  <Input
                    value={bm25TextField}
                    onChange={(e) => setBm25TextField(e.target.value)}
                    placeholder="text"
                  />
                </div>
              </div>
            )}
          </div>

          {/* ─── Tiered Storage ─────────────────────────────── */}
          <div className="border-t border-white/[0.06] pt-4">
            <label className="mb-3 flex cursor-pointer items-center gap-2 text-sm font-medium">
              <input
                type="checkbox"
                checked={enableTiered}
                onChange={(e) => setEnableTiered(e.target.checked)}
                className="rounded border-white/[0.2] bg-white/[0.04] text-orange-500 focus:ring-orange-500"
              />
              Enable Tiered Storage
            </label>
            {enableTiered && (
              <div className="ml-6 space-y-3 rounded-lg border border-white/[0.06] bg-white/[0.03] p-3">
                <p className="text-xs text-gray-400">
                  Automatically moves vectors between Hot (RAM), Warm (mmap), and Cold (disk) tiers based on access frequency.
                </p>
                <div className="grid grid-cols-1 gap-3">
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Hot Threshold (hours)</label>
                    <Input
                      type="number"
                      value={hotThreshold}
                      onChange={(e) => setHotThreshold(e.target.value)}
                      placeholder="24"
                    />
                    <p className="mt-1 text-[10px] text-gray-500">Points accessed within this time stay in RAM</p>
                  </div>
                  <div>
                    <label className="mb-1 block text-xs text-gray-400">Warm Threshold (hours)</label>
                    <Input
                      type="number"
                      value={warmThreshold}
                      onChange={(e) => setWarmThreshold(e.target.value)}
                      placeholder="168"
                    />
                    <p className="mt-1 text-[10px] text-gray-500">Points accessed within this time use memory-mapped files</p>
                  </div>
                  <div>
                    <label className="mb-1 block text-xs text-gray-400">Compaction Interval (seconds)</label>
                    <Input
                      type="number"
                      value={compactionInterval}
                      onChange={(e) => setCompactionInterval(e.target.value)}
                      placeholder="3600"
                    />
                    <p className="text-[10px] text-gray-500 mt-1">How often the background task checks for tier demotions</p>
                  </div>
                </div>
              </div>
            )}
          </div>

          <div className="flex justify-end gap-2 border-t border-white/[0.06] pt-4">
            <Button variant="secondary" onClick={() => setIsCreateModalOpen(false)}>
              Cancel
            </Button>
            <Button onClick={handleCreate} disabled={!newCollectionName || !newVectorSize}>
              Create
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
};
