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

  const handleCreate = async () => {
    if (!newCollectionName || !newVectorSize) return;
    await createCollection.mutateAsync({
      name: newCollectionName,
      vectorSize: parseInt(newVectorSize),
      distanceMetric: newDistanceMetric,
    });
    setIsCreateModalOpen(false);
    setNewCollectionName('');
    setNewVectorSize('128');
  };

  const handleDelete = async (name: string) => {
    if (confirm(`Are you sure you want to delete collection "${name}"?`)) {
      await deleteCollection.mutateAsync(name);
    }
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold">Collections</h1>
          <p className="text-gray-600 mt-2">Manage your vector collections</p>
        </div>
        {canEdit && (
          <Button onClick={() => setIsCreateModalOpen(true)}>
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
                    <TableCell colSpan={5} className="text-center text-gray-500">
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
        <div className="space-y-4">
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
              className="flex h-10 w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 ring-offset-bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-orange-500 focus-visible:ring-offset-2 [&>option]:bg-bg-secondary [&>option]:text-gray-50"
              value={newDistanceMetric}
              onChange={(e) => setNewDistanceMetric(e.target.value)}
            >
              <option value="cosine">Cosine</option>
              <option value="euclidean">Euclidean</option>
              <option value="dot">Dot Product</option>
            </select>
          </div>
          <div className="flex justify-end gap-2">
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
