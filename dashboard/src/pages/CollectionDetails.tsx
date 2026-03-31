import { useParams, useNavigate } from 'react-router-dom';
import { useCollection } from '@/hooks/useCollections';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Skeleton } from '@/components/ui/Skeleton';
import { ArrowLeft, FileText, Network } from 'lucide-react';

export const CollectionDetails = () => {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const { data: collection, isLoading } = useCollection(name || '');

  if (!name) {
    return (
      <div className="space-y-6">
        <Card>
          <CardContent className="pt-6">
            <p className="text-gray-400">Collection name is required</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="sm" onClick={() => navigate('/collections')}>
          <ArrowLeft className="h-4 w-4 mr-2" />
          Back
        </Button>
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-gray-50">{name}</h1>
          <p className="mt-1 text-sm text-gray-400">Choose a view for this collection</p>
        </div>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Collection</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          {isLoading ? (
            <Skeleton className="h-10 w-64" />
          ) : (
            <p className="text-sm text-gray-400">
              {collection?.num_points ?? 0} points · dimension {collection?.dimension ?? '—'}
            </p>
          )}
          <div className="flex flex-wrap gap-3 pt-2">
            <Button
              variant="primary"
              size="md"
              onClick={() => navigate(`/collections/${name}/points`)}
              className="gap-2"
            >
              <FileText className="h-4 w-4" />
              View Points
            </Button>
            <Button
              variant="secondary"
              size="md"
              onClick={() => navigate(`/collections/${name}/graph`)}
              className="gap-2"
            >
              <Network className="h-4 w-4" />
              View Graph
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  );
};
