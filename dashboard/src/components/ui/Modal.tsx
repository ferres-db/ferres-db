import type { ReactNode } from 'react';
import { X } from 'lucide-react';
import { Button } from '@/components/ui/Button';
import { cn } from '@/utils/cn';

export interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  children: ReactNode;
  className?: string;
}

export const Modal = ({ isOpen, onClose, title, children, className }: ModalProps) => {
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div
        className="fixed inset-0 bg-black/60 backdrop-blur-sm animate-fade-in"
        onClick={onClose}
        aria-hidden
      />
      <div
        role="dialog"
        aria-modal="true"
        className={cn(
          'relative z-50 w-full max-w-lg rounded-xl border border-black/30 bg-bg-secondary shadow-modal animate-fade-in',
          className
        )}
      >
        {title && (
          <div className="flex items-center justify-between border-b border-black/20 px-6 py-4">
            <h2 className="text-lg font-semibold text-gray-50">{title}</h2>
            <Button variant="ghost" size="sm" onClick={onClose} className="h-8 w-8 rounded-lg p-0">
              <X className="h-4 w-4" />
            </Button>
          </div>
        )}
        <div className={title ? 'p-6' : 'p-6'}>{children}</div>
      </div>
    </div>
  );
};
