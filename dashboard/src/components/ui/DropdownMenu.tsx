import type { ReactNode } from 'react';
import { useState, useRef, useEffect } from 'react';
import { MoreVertical } from 'lucide-react';
import { cn } from '@/utils/cn';
import { Button } from './Button';

export interface DropdownMenuProps {
  children: ReactNode;
  trigger?: ReactNode;
}

export const DropdownMenu = ({ children, trigger }: DropdownMenuProps) => {
  const [isOpen, setIsOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    };

    if (isOpen) {
      document.addEventListener('mousedown', handleClickOutside);
    }

    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
    };
  }, [isOpen]);

  return (
    <div className="relative" ref={menuRef}>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setIsOpen(!isOpen)}
        className="h-8 w-8 p-0"
      >
        {trigger || <MoreVertical className="h-4 w-4" />}
      </Button>
      {isOpen && (
        <div className="absolute right-0 mt-2 w-48 rounded-md border border-bg-tertiary bg-bg-secondary shadow-lg z-50">
          <div className="py-1" onClick={() => setIsOpen(false)}>
            {children}
          </div>
        </div>
      )}
    </div>
  );
};

export interface DropdownMenuItemProps {
  children: ReactNode;
  onClick?: () => void;
  className?: string;
}

export const DropdownMenuItem = ({ children, onClick, className }: DropdownMenuItemProps) => {
  return (
    <button
      onClick={onClick}
      className={cn(
        'w-full text-left px-4 py-2 text-sm text-gray-50 hover:bg-bg-tertiary transition-colors',
        className
      )}
    >
      {children}
    </button>
  );
};
