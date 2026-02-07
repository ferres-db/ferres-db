import { useParams, useNavigate } from 'react-router-dom';
import { useState, useMemo } from 'react';
import { useCollection, useTierDistribution } from '@/hooks/useCollections';
import { usePoints } from '@/hooks/usePoints';
import { useCollectionStats, useCollectionQueries } from '@/hooks/useCollectionStats';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Skeleton } from '@/components/ui/Skeleton';
import { Badge } from '@/components/ui/Badge';
import { DropdownMenu, DropdownMenuItem } from '@/components/ui/DropdownMenu';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/Tabs';
import { ArrowLeft, FileText, Copy, Eye, ChevronLeft, ChevronRight, X, Plus, BarChart3, Layers } from 'lucide-react';
import { format } from 'date-fns';
import { LineChart, Line, BarChart, Bar, Cell, XAxis, YAxis, CartesianGrid, Tooltip, Legend, ResponsiveContainer } from 'recharts';

interface FilterField {
  id: string;
  field: string;
  operator: '$eq' | '$ne' | '$in' | '$gt' | '$lt' | '$gte' | '$lte';
  value: string;
}

export const CollectionPoints = () => {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useState('browser');
  const [expandedPoint, setExpandedPoint] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [limit, setLimit] = useState(10);
  const [showFilterModal, setShowFilterModal] = useState(false);
  const [filterFields, setFilterFields] = useState<FilterField[]>([]);
  const [newFilter, setNewFilter] = useState<FilterField>({ id: '', field: '', operator: '$eq', value: '' });
  
  const { data: collection, isLoading: collectionLoading } = useCollection(name || '');
  const { data: stats, isLoading: statsLoading } = useCollectionStats(name || '');
  const { data: queries, isLoading: queriesLoading } = useCollectionQueries(name || '');
  const { data: tierDistribution } = useTierDistribution(name || '');
  
  // Construir filtro a partir dos campos de filtro
  const filter = useMemo(() => {
    if (filterFields.length === 0) return undefined;
    
    const filterObj: Record<string, unknown> = {};
    filterFields.forEach((f) => {
      if (!f.field.trim()) return;
      
      let value: unknown = f.value.trim();
      
      // Converter valor baseado no tipo
      if (f.operator === '$in') {
        value = f.value.split(',').map(v => v.trim()).filter(v => v);
      } else if (['$gt', '$lt', '$gte', '$lte'].includes(f.operator)) {
        const num = parseFloat(f.value);
        if (isNaN(num)) return;
        value = num;
      } else if (f.value.toLowerCase() === 'true') {
        value = true;
      } else if (f.value.toLowerCase() === 'false') {
        value = false;
      } else if (!isNaN(Number(f.value)) && f.value.trim() !== '') {
        value = Number(f.value);
      }
      
      if (f.operator === '$eq') {
        filterObj[f.field] = value;
      } else {
        if (!filterObj[f.field]) {
          filterObj[f.field] = {};
        }
        (filterObj[f.field] as Record<string, unknown>)[f.operator] = value;
      }
    });
    
    return Object.keys(filterObj).length > 0 ? filterObj : undefined;
  }, [filterFields]);
  
  const { data: points, isLoading: pointsLoading } = usePoints(name || '', {
    limit,
    offset: page * limit,
    filter,
  });
  
  const totalPages = points ? Math.ceil(points.total / limit) : 0;

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text);
  };

  const formatMetadataValue = (value: unknown): string => {
    if (value === null || value === undefined) return 'null';
    if (typeof value === 'object') return JSON.stringify(value);
    return String(value);
  };

  const getTextFromMetadata = (metadata?: Record<string, unknown>): string | null => {
    if (!metadata) return null;
    return (metadata.text || metadata.content || metadata.body) as string | null;
  };

  const addFilter = () => {
    if (!newFilter.field.trim() || !newFilter.value.trim()) return;
    
    const filterToAdd = {
      ...newFilter,
      id: Date.now().toString(),
    };
    
    setFilterFields([...filterFields, filterToAdd]);
    setNewFilter({ id: '', field: '', operator: '$eq', value: '' });
    setShowFilterModal(false);
    setPage(0);
  };

  const removeFilterField = (id: string) => {
    setFilterFields(filterFields.filter(f => f.id !== id));
    setPage(0);
  };

  const clearFilters = () => {
    setFilterFields([]);
    setPage(0);
  };

  const getFilterDisplayText = (filter: FilterField): string => {
    const operatorText: Record<string, string> = {
      '$eq': '=',
      '$ne': '≠',
      '$in': 'in',
      '$gt': '>',
      '$gte': '≥',
      '$lt': '<',
      '$lte': '≤',
    };
    
    return `${filter.field} ${operatorText[filter.operator] || filter.operator} ${filter.value}`;
  };

  // Preparar dados para gráficos
  const latencyChartData = useMemo(() => {
    if (!queries || queries.length === 0) return [];
    
    // Agrupar por hora e calcular média de latência
    const hourlyData: Record<string, { count: number; totalLatency: number }> = {};
    
    queries.forEach((query) => {
      const date = new Date(query.timestamp);
      const hour = `${date.getHours()}:00`;
      
      if (!hourlyData[hour]) {
        hourlyData[hour] = { count: 0, totalLatency: 0 };
      }
      
      hourlyData[hour].count += 1;
      hourlyData[hour].totalLatency += query.took_ms;
    });
    
    return Object.entries(hourlyData)
      .map(([hour, data]) => ({
        hour,
        avgLatency: Math.round((data.totalLatency / data.count) * 100) / 100,
        count: data.count,
      }))
      .sort((a, b) => a.hour.localeCompare(b.hour))
      .slice(-24); // Últimas 24 horas
  }, [queries]);

  const percentileData = useMemo(() => {
    if (!stats) return null;
    
    return [
      { name: 'P50', value: stats.p50_latency_ms },
      { name: 'P95', value: stats.p95_latency_ms },
      { name: 'P99', value: stats.p99_latency_ms },
    ];
  }, [stats]);

  const MetadataField = ({ field, value, isLong }: { field: string; value: string; isLong: boolean }) => {
    const [isExpanded, setIsExpanded] = useState(false);
    
    return (
      <div className="flex gap-2 text-sm">
        <span className="text-gray-400 font-medium min-w-[140px] flex-shrink-0">{field}:</span>
        <span className="text-gray-200 break-words flex-1">
          {isLong && !isExpanded ? (
            <>
              {value.substring(0, 100)}...
              <button
                onClick={() => setIsExpanded(true)}
                className="text-orange-500 hover:text-orange-400 ml-1 text-xs"
              >
                show more
              </button>
            </>
          ) : (
            <>
              {value}
              {isLong && (
                <button
                  onClick={() => setIsExpanded(false)}
                  className="text-orange-500 hover:text-orange-400 ml-1 text-xs"
                >
                  show less
                </button>
              )}
            </>
          )}
        </span>
      </div>
    );
  };

  const formatBytes = (bytes: number): string => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
  };

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
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <Button variant="ghost" size="sm" onClick={() => navigate('/collections')}>
            <ArrowLeft className="h-4 w-4 mr-2" />
            Back
          </Button>
          <div>
            <h1 className="text-3xl font-bold text-gray-50">{name}</h1>
            <p className="text-gray-400 mt-2">Collection details and points</p>
          </div>
        </div>
      </div>

      {/* Collection Info */}
      <Card>
        <CardHeader>
          <CardTitle>Collection Information</CardTitle>
        </CardHeader>
        <CardContent>
          {collectionLoading ? (
            <div className="space-y-2">
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-full" />
            </div>
          ) : (
            <>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
              <div>
                <p className="text-sm text-gray-400">Dimension</p>
                <p className="text-lg font-semibold text-gray-50">{collection?.dimension ?? 'N/A'}</p>
              </div>
              <div>
                <p className="text-sm text-gray-400">Total Points</p>
                <p className="text-lg font-semibold text-gray-50">{collection?.num_points ?? 0}</p>
              </div>
              <div>
                <p className="text-sm text-gray-400">Distance Metric</p>
                <Badge variant="default">{collection?.distance ?? 'N/A'}</Badge>
              </div>
              <div>
                <p className="text-sm text-gray-400">Created At</p>
                <p className="text-sm text-gray-50">
                  {collection?.created_at
                    ? format(new Date(collection.created_at * 1000), 'MMM dd, yyyy')
                    : 'N/A'}
                </p>
              </div>
            </div>

            {/* Tier Distribution */}
            {tierDistribution && (tierDistribution.warm > 0 || tierDistribution.cold > 0) && (
              <div className="mt-4 pt-4 border-t border-bg-tertiary">
                <div className="flex items-center gap-2 mb-3">
                  <Layers className="h-4 w-4 text-gray-400" />
                  <p className="text-sm font-medium text-gray-400">Tiered Storage</p>
                </div>
                <div className="grid grid-cols-3 gap-4">
                  <div className="bg-bg-tertiary/50 rounded-md p-3">
                    <div className="flex items-center gap-2 mb-1">
                      <div className="w-2 h-2 rounded-full bg-orange-500" />
                      <p className="text-xs text-gray-400">Hot (RAM)</p>
                    </div>
                    <p className="text-lg font-semibold text-gray-50">{tierDistribution.hot.toLocaleString()}</p>
                    <p className="text-[10px] text-gray-500">{formatBytes(tierDistribution.hot_memory_bytes)}</p>
                  </div>
                  <div className="bg-bg-tertiary/50 rounded-md p-3">
                    <div className="flex items-center gap-2 mb-1">
                      <div className="w-2 h-2 rounded-full bg-yellow-500" />
                      <p className="text-xs text-gray-400">Warm (mmap)</p>
                    </div>
                    <p className="text-lg font-semibold text-gray-50">{tierDistribution.warm.toLocaleString()}</p>
                    <p className="text-[10px] text-gray-500">{formatBytes(tierDistribution.warm_memory_bytes)}</p>
                  </div>
                  <div className="bg-bg-tertiary/50 rounded-md p-3">
                    <div className="flex items-center gap-2 mb-1">
                      <div className="w-2 h-2 rounded-full bg-blue-500" />
                      <p className="text-xs text-gray-400">Cold (disk)</p>
                    </div>
                    <p className="text-lg font-semibold text-gray-50">{tierDistribution.cold.toLocaleString()}</p>
                    <p className="text-[10px] text-gray-500">{formatBytes(tierDistribution.cold_memory_bytes)}</p>
                  </div>
                </div>
                {/* Tier distribution bar */}
                {(tierDistribution.hot + tierDistribution.warm + tierDistribution.cold) > 0 && (
                  <div className="mt-3 h-2 rounded-full overflow-hidden flex bg-bg-tertiary">
                    {tierDistribution.hot > 0 && (
                      <div
                        className="bg-orange-500 h-full"
                        style={{ width: `${(tierDistribution.hot / (tierDistribution.hot + tierDistribution.warm + tierDistribution.cold)) * 100}%` }}
                      />
                    )}
                    {tierDistribution.warm > 0 && (
                      <div
                        className="bg-yellow-500 h-full"
                        style={{ width: `${(tierDistribution.warm / (tierDistribution.hot + tierDistribution.warm + tierDistribution.cold)) * 100}%` }}
                      />
                    )}
                    {tierDistribution.cold > 0 && (
                      <div
                        className="bg-blue-500 h-full"
                        style={{ width: `${(tierDistribution.cold / (tierDistribution.hot + tierDistribution.warm + tierDistribution.cold)) * 100}%` }}
                      />
                    )}
                  </div>
                )}
              </div>
            )}
            </>
          )}
        </CardContent>
      </Card>

      {/* Tabs */}
      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList>
          <TabsTrigger value="browser">
            <FileText className="h-4 w-4 mr-2" />
            BROWSER
          </TabsTrigger>
          <TabsTrigger value="metrics">
            <BarChart3 className="h-4 w-4 mr-2" />
            METRICS
          </TabsTrigger>
        </TabsList>

        {/* Browser Tab */}
        <TabsContent value="browser">
          <div>
            <div className="flex items-center justify-between mb-4">
              <div className="flex items-center gap-2">
                <FileText className="h-5 w-5 text-gray-400" />
                <h2 className="text-lg font-semibold text-gray-50">
                  Points: {points?.total ?? 0} results
                  {points && points.limit && (
                    <span className="text-sm text-gray-400 font-normal ml-2">
                      (showing {points.points.length} of {points.total})
                    </span>
                  )}
                </h2>
              </div>
              <div className="flex items-center gap-2">
                <div className="flex items-center gap-2">
                  <label className="text-sm text-gray-400">Limit:</label>
                  <select
                    value={limit}
                    onChange={(e) => {
                      setLimit(Number(e.target.value));
                      setPage(0);
                    }}
                    className="h-8 rounded-md border border-bg-tertiary bg-bg-secondary px-2 text-sm text-gray-50"
                  >
                    <option value={10}>10</option>
                    <option value={25}>25</option>
                    <option value={50}>50</option>
                    <option value={100}>100</option>
                  </select>
                </div>
              </div>
            </div>

            {/* Active Filters */}
            <div className="flex items-center gap-2 mb-4 flex-wrap">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setShowFilterModal(true)}
                className="h-8"
              >
                <Plus className="h-4 w-4 mr-1" />
                Filter
              </Button>
              
              {filterFields.map((filter) => (
                <Badge
                  key={filter.id}
                  variant="default"
                  className="flex items-center gap-1 px-2 py-1 h-8"
                >
                  <span className="text-xs">{getFilterDisplayText(filter)}</span>
                  <button
                    onClick={() => removeFilterField(filter.id)}
                    className="ml-1 hover:text-gray-50"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </Badge>
              ))}
              
              {filterFields.length > 0 && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={clearFilters}
                  className="h-8 text-xs text-gray-400 hover:text-gray-50"
                >
                  Clear all
                </Button>
              )}
            </div>

            {/* Filter Modal */}
            {showFilterModal && (
              <Card className="mb-4 border-orange-500/50">
                <CardContent className="p-4">
                  <div className="space-y-3">
                    <div className="flex items-center justify-between">
                      <h3 className="text-sm font-semibold text-gray-50">Add Filter</h3>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          setShowFilterModal(false);
                          setNewFilter({ id: '', field: '', operator: '$eq', value: '' });
                        }}
                        className="h-6 w-6 p-0"
                      >
                        <X className="h-4 w-4" />
                      </Button>
                    </div>
                    
                    <div className="grid grid-cols-12 gap-2">
                      <div className="col-span-4">
                        <Input
                          placeholder="Field name"
                          value={newFilter.field}
                          onChange={(e) => setNewFilter({ ...newFilter, field: e.target.value })}
                          className="h-9 text-sm"
                        />
                      </div>
                      <div className="col-span-3">
                        <select
                          value={newFilter.operator}
                          onChange={(e) => setNewFilter({ ...newFilter, operator: e.target.value as FilterField['operator'] })}
                          className="h-9 w-full rounded-md border border-bg-tertiary bg-bg-secondary px-2 text-sm text-gray-50"
                        >
                          <option value="$eq">Equals</option>
                          <option value="$ne">Not equals</option>
                          <option value="$in">In</option>
                          <option value="$gt">&gt;</option>
                          <option value="$gte">≥</option>
                          <option value="$lt">&lt;</option>
                          <option value="$lte">≤</option>
                        </select>
                      </div>
                      <div className="col-span-4">
                        <Input
                          placeholder={newFilter.operator === '$in' ? 'value1, value2' : 'Value'}
                          value={newFilter.value}
                          onChange={(e) => setNewFilter({ ...newFilter, value: e.target.value })}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') {
                              addFilter();
                            }
                          }}
                          className="h-9 text-sm"
                        />
                      </div>
                      <div className="col-span-1">
                        <Button
                          variant="primary"
                          size="sm"
                          onClick={addFilter}
                          disabled={!newFilter.field.trim() || !newFilter.value.trim()}
                          className="h-9 w-full"
                        >
                          Add
                        </Button>
                      </div>
                    </div>
                  </div>
                </CardContent>
              </Card>
            )}

            {pointsLoading ? (
              <div className="space-y-4">
                {[1, 2, 3].map((i) => (
                  <Skeleton key={i} className="h-48 w-full" />
                ))}
              </div>
            ) : points && Array.isArray(points.points) && points.points.length > 0 ? (
              <div className="space-y-4">
                {points.points.map((point, index) => {
                  const textContent = getTextFromMetadata(point.metadata);
                  const isExpanded = expandedPoint === point.id;
                  const metadataEntries = point.metadata ? Object.entries(point.metadata) : [];
                  const globalIndex = page * limit + index + 1;

                  return (
                    <Card key={point.id} className="hover:border-orange-500/50 transition-colors">
                      <CardContent className="p-6">
                        <div className="flex gap-4">
                          <div className="flex-shrink-0">
                            <div className="w-12 h-12 rounded-full bg-bg-tertiary flex items-center justify-center">
                              <span className="text-xl font-semibold text-gray-50">{globalIndex}</span>
                            </div>
                            {point.vector && (
                              <div className="mt-2 text-center">
                                <div className="text-xs text-gray-400 mb-1">DIM</div>
                                <Badge variant="default" className="text-xs">
                                  {point.vector.length}
                                </Badge>
                              </div>
                            )}
                          </div>

                          <div className="flex-1 min-w-0">
                            <div className="mb-3">
                              <div className="text-xs text-gray-400 mb-1">ID</div>
                              <div className="font-mono text-sm text-gray-50 break-all">{point.id}</div>
                            </div>

                            {textContent && (
                              <div className="mb-4">
                                <div className="text-xs text-gray-400 mb-1">text</div>
                                <div className="text-sm text-gray-200 bg-bg-tertiary p-3 rounded border border-bg-tertiary">
                                  {isExpanded ? (
                                    <div className="whitespace-pre-wrap">{textContent}</div>
                                  ) : (
                                    <div className="line-clamp-3">{textContent}</div>
                                  )}
                                  {textContent.length > 200 && (
                                    <button
                                      onClick={() => setExpandedPoint(isExpanded ? null : point.id)}
                                      className="text-xs text-orange-500 hover:text-orange-400 mt-2"
                                    >
                                      {isExpanded ? 'Show less' : 'Show more'}
                                    </button>
                                  )}
                                </div>
                              </div>
                            )}

                            {metadataEntries.length > 0 && (
                              <div className="space-y-2 mt-4">
                                <div className="text-xs text-gray-400 mb-2">Metadata:</div>
                                <div className="bg-bg-tertiary/50 rounded p-3 space-y-2">
                                  {metadataEntries
                                    .filter(([key]) => key !== 'text' && key !== 'content' && key !== 'body')
                                    .map(([key, value]) => {
                                      const formattedValue = formatMetadataValue(value);
                                      const isLong = formattedValue.length > 100;
                                      
                                      return (
                                        <MetadataField
                                          key={key}
                                          field={key}
                                          value={formattedValue}
                                          isLong={isLong}
                                        />
                                      );
                                    })}
                                </div>
                              </div>
                            )}

                            {point.created_at && (
                              <div className="mt-3 pt-3 border-t border-bg-tertiary">
                                <div className="text-xs text-gray-400">
                                  Created: {format(new Date(point.created_at * 1000), 'MMM dd, yyyy HH:mm')}
                                </div>
                              </div>
                            )}
                          </div>

                          <div className="flex-shrink-0">
                            <DropdownMenu>
                              <DropdownMenuItem onClick={() => copyToClipboard(point.id)}>
                                <Copy className="h-4 w-4 mr-2 inline" />
                                Copy ID
                              </DropdownMenuItem>
                              <DropdownMenuItem onClick={() => copyToClipboard(JSON.stringify(point, null, 2))}>
                                <Copy className="h-4 w-4 mr-2 inline" />
                                Copy JSON
                              </DropdownMenuItem>
                              <DropdownMenuItem onClick={() => navigator.clipboard.writeText(JSON.stringify(point.metadata || {}, null, 2))}>
                                <Eye className="h-4 w-4 mr-2 inline" />
                                View Metadata
                              </DropdownMenuItem>
                            </DropdownMenu>
                          </div>
                        </div>
                      </CardContent>
                    </Card>
                  );
                })}
              </div>
            ) : (
              <Card>
                <CardContent className="p-12 text-center">
                  <p className="text-gray-400">No points found</p>
                </CardContent>
              </Card>
            )}

            {points && totalPages > 1 && (
              <div className="flex items-center justify-center mt-6 gap-4">
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => setPage(Math.max(0, page - 1))}
                  disabled={page === 0}
                >
                  <ChevronLeft className="h-4 w-4 mr-1" />
                  Previous
                </Button>
                <div className="text-sm text-gray-400 px-4">
                  Page {page + 1} of {totalPages} ({points.total} total)
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => setPage(Math.min(totalPages - 1, page + 1))}
                  disabled={page >= totalPages - 1}
                >
                  Next
                  <ChevronRight className="h-4 w-4 ml-1" />
                </Button>
              </div>
            )}
          </div>
        </TabsContent>

        {/* Metrics Tab */}
        <TabsContent value="metrics">
          <div className="space-y-6">
            {statsLoading ? (
              <div className="space-y-4">
                <Skeleton className="h-64 w-full" />
                <Skeleton className="h-64 w-full" />
              </div>
            ) : stats ? (
              <>
                {/* Stats Overview */}
                <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                  <Card>
                    <CardHeader>
                      <CardTitle className="text-sm font-medium text-gray-400">Total Queries</CardTitle>
                    </CardHeader>
                    <CardContent>
                      <p className="text-3xl font-bold text-gray-50">{stats.num_queries.toLocaleString()}</p>
                    </CardContent>
                  </Card>
                  <Card>
                    <CardHeader>
                      <CardTitle className="text-sm font-medium text-gray-400">Average Latency</CardTitle>
                    </CardHeader>
                    <CardContent>
                      <p className="text-3xl font-bold text-gray-50">{stats.avg_latency_ms.toFixed(2)}ms</p>
                    </CardContent>
                  </Card>
                  <Card>
                    <CardHeader>
                      <CardTitle className="text-sm font-medium text-gray-400">P95 Latency</CardTitle>
                    </CardHeader>
                    <CardContent>
                      <p className="text-3xl font-bold text-gray-50">{stats.p95_latency_ms.toFixed(2)}ms</p>
                    </CardContent>
                  </Card>
                </div>

                {/* Tier Distribution Chart */}
                {tierDistribution && (tierDistribution.warm > 0 || tierDistribution.cold > 0) && (
                  <Card>
                    <CardHeader>
                      <CardTitle className="flex items-center gap-2">
                        <Layers className="h-5 w-5" />
                        Tiered Storage Distribution
                      </CardTitle>
                    </CardHeader>
                    <CardContent>
                      <ResponsiveContainer width="100%" height={250}>
                        <BarChart
                          data={[
                            { name: 'Hot (RAM)', points: tierDistribution.hot, memory: tierDistribution.hot_memory_bytes, fill: '#f97316' },
                            { name: 'Warm (mmap)', points: tierDistribution.warm, memory: tierDistribution.warm_memory_bytes, fill: '#eab308' },
                            { name: 'Cold (disk)', points: tierDistribution.cold, memory: tierDistribution.cold_memory_bytes, fill: '#3b82f6' },
                          ]}
                        >
                          <CartesianGrid strokeDasharray="3 3" stroke="#3f3f3f" />
                          <XAxis dataKey="name" stroke="#9ca3af" />
                          <YAxis stroke="#9ca3af" />
                          <Tooltip
                            contentStyle={{ backgroundColor: '#2d2d2d', border: '1px solid #3f3f3f', borderRadius: '8px' }}
                            labelStyle={{ color: '#f9fafb' }}
                            formatter={((value: any, _name: any, props: any) => {
                              const memBytes = props?.payload?.memory ?? 0;
                              return [`${Number(value).toLocaleString()} points (${formatBytes(memBytes)})`, 'Count'];
                            }) as any}
                          />
                          <Bar dataKey="points">
                            {['#f97316', '#eab308', '#3b82f6'].map((color, index) => (
                              <Cell key={`cell-${index}`} fill={color} />
                            ))}
                          </Bar>
                        </BarChart>
                      </ResponsiveContainer>
                    </CardContent>
                  </Card>
                )}

                {/* Percentiles Chart */}
                {percentileData && (
                  <Card>
                    <CardHeader>
                      <CardTitle>Latency Percentiles</CardTitle>
                    </CardHeader>
                    <CardContent>
                      <ResponsiveContainer width="100%" height={300}>
                        <BarChart data={percentileData}>
                          <CartesianGrid strokeDasharray="3 3" stroke="#3f3f3f" />
                          <XAxis dataKey="name" stroke="#9ca3af" />
                          <YAxis stroke="#9ca3af" />
                          <Tooltip 
                            contentStyle={{ backgroundColor: '#2d2d2d', border: '1px solid #3f3f3f', borderRadius: '8px' }}
                            labelStyle={{ color: '#f9fafb' }}
                          />
                          <Bar dataKey="value" fill="#f97316" />
                        </BarChart>
                      </ResponsiveContainer>
                    </CardContent>
                  </Card>
                )}

                {/* Latency Over Time */}
                {queriesLoading ? (
                  <Skeleton className="h-64 w-full" />
                ) : latencyChartData.length > 0 ? (
                  <Card>
                    <CardHeader>
                      <CardTitle>Query Latency Over Time</CardTitle>
                    </CardHeader>
                    <CardContent>
                      <ResponsiveContainer width="100%" height={300}>
                        <LineChart data={latencyChartData}>
                          <CartesianGrid strokeDasharray="3 3" stroke="#3f3f3f" />
                          <XAxis dataKey="hour" stroke="#9ca3af" />
                          <YAxis stroke="#9ca3af" />
                          <Tooltip 
                            contentStyle={{ backgroundColor: '#2d2d2d', border: '1px solid #3f3f3f', borderRadius: '8px' }}
                            labelStyle={{ color: '#f9fafb' }}
                          />
                          <Legend />
                          <Line type="monotone" dataKey="avgLatency" stroke="#f97316" strokeWidth={2} name="Avg Latency (ms)" />
                        </LineChart>
                      </ResponsiveContainer>
                    </CardContent>
                  </Card>
                ) : (
                  <Card>
                    <CardContent className="p-12 text-center">
                      <p className="text-gray-400">No query data available</p>
                    </CardContent>
                  </Card>
                )}
              </>
            ) : (
              <Card>
                <CardContent className="p-12 text-center">
                  <p className="text-gray-400">No metrics available</p>
                </CardContent>
              </Card>
            )}
          </div>
        </TabsContent>
      </Tabs>
    </div>
  );
};
