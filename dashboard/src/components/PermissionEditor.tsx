import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import type { Permission, Resource, Action } from '@/types';
import { Plus, Trash2 } from 'lucide-react';

const ACTIONS: Action[] = ['read', 'write', 'create', 'delete', 'admin'];

interface PermissionEditorProps {
  value: Permission[];
  onChange: (value: Permission[]) => void;
  disabled?: boolean;
}

function defaultPermission(): Permission {
  return {
    resource: { type: 'collection', name: '' },
    actions: ['read'],
    metadata_restriction: undefined,
  };
}

export const PermissionEditor = ({ value, onChange, disabled }: PermissionEditorProps) => {
  const add = () => onChange([...value, defaultPermission()]);
  const remove = (i: number) => onChange(value.filter((_, j) => j !== i));
  const update = (i: number, p: Permission) => {
    const next = [...value];
    next[i] = p;
    onChange(next);
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <span className="text-sm font-medium text-gray-300">Granular permissions</span>
        {!disabled && (
          <Button type="button" variant="secondary" size="sm" onClick={add}>
            <Plus className="h-4 w-4 mr-1" />
            Add
          </Button>
        )}
      </div>
      {value.length === 0 && (
        <p className="text-sm text-gray-500">None. Access uses the role only (viewer/editor/admin).</p>
      )}
      {value.map((perm, i) => (
        <div
          key={i}
          className="rounded-lg border border-bg-tertiary bg-bg-secondary/50 p-3 space-y-2"
        >
          <div className="flex justify-between items-start gap-2">
            <div className="flex-1 grid grid-cols-1 sm:grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-gray-500 block mb-1">Resource</label>
                <select
                  className="flex h-9 w-full rounded-md border border-bg-tertiary bg-bg-secondary px-2 text-sm text-gray-50"
                  value={perm.resource.type === 'collection' ? 'collection' : 'all_collections'}
                  onChange={(e) => {
                    const type = e.target.value as Resource['type'];
                    update(i, {
                      ...perm,
                      resource:
                        type === 'all_collections'
                          ? { type: 'all_collections' }
                          : { type: 'collection', name: perm.resource.type === 'collection' ? perm.resource.name : '' },
                    });
                  }}
                  disabled={disabled}
                >
                  <option value="all_collections">All collections</option>
                  <option value="collection">Specific collection</option>
                </select>
              </div>
              {perm.resource.type === 'collection' && (
                <div>
                  <label className="text-xs text-gray-500 block mb-1">Collection name</label>
                  <Input
                    value={perm.resource.name}
                    onChange={(e) =>
                      update(i, {
                        ...perm,
                        resource: { type: 'collection', name: e.target.value.trim() },
                      })
                    }
                    placeholder="e.g. docs"
                    disabled={disabled}
                    className="h-9"
                  />
                </div>
              )}
            </div>
            {!disabled && (
              <Button type="button" variant="ghost" size="sm" onClick={() => remove(i)} title="Remove">
                <Trash2 className="h-4 w-4 text-gray-400 hover:text-red-400" />
              </Button>
            )}
          </div>
          <div>
            <label className="text-xs text-gray-500 block mb-1">Actions</label>
            <div className="flex flex-wrap gap-2">
              {ACTIONS.map((action) => (
                <label key={action} className="flex items-center gap-1.5 text-sm">
                  <input
                    type="checkbox"
                    checked={perm.actions.includes(action)}
                    onChange={(e) => {
                      const next = e.target.checked
                        ? [...perm.actions, action]
                        : perm.actions.filter((a) => a !== action);
                      update(i, { ...perm, actions: next });
                    }}
                    disabled={disabled}
                    className="rounded border-bg-tertiary"
                  />
                  <span className="capitalize">{action}</span>
                </label>
              ))}
            </div>
          </div>
          <div className="flex items-center gap-2">
            <label className="flex items-center gap-1.5 text-sm">
              <input
                type="checkbox"
                checked={!!perm.metadata_restriction}
                onChange={(e) => {
                  update(i, {
                    ...perm,
                    metadata_restriction: e.target.checked
                      ? { field: '', allowed_values: [] }
                      : undefined,
                  });
                }}
                disabled={disabled}
                className="rounded border-bg-tertiary"
              />
              Metadata restriction
            </label>
          </div>
          {perm.metadata_restriction && (
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 pt-1 border-t border-bg-tertiary">
              <div>
                <label className="text-xs text-gray-500 block mb-1">Field</label>
                <Input
                  value={perm.metadata_restriction.field}
                  onChange={(e) =>
                    update(i, {
                      ...perm,
                      metadata_restriction: perm.metadata_restriction
                        ? { ...perm.metadata_restriction, field: e.target.value }
                        : undefined,
                    })
                  }
                  placeholder="e.g. department"
                  disabled={disabled}
                  className="h-9"
                />
              </div>
              <div>
                <label className="text-xs text-gray-500 block mb-1">Allowed values (JSON array)</label>
                <Input
                  value={JSON.stringify(perm.metadata_restriction.allowed_values)}
                  onChange={(e) => {
                    try {
                      const arr = JSON.parse(e.target.value || '[]');
                      if (Array.isArray(arr) && perm.metadata_restriction) {
                        update(i, {
                          ...perm,
                          metadata_restriction: { ...perm.metadata_restriction, allowed_values: arr },
                        });
                      }
                    } catch {
                      // ignore invalid json while typing
                    }
                  }}
                  placeholder='["sales"]'
                  disabled={disabled}
                  className="h-9 font-mono text-sm"
                />
              </div>
            </div>
          )}
        </div>
      ))}
    </div>
  );
};
