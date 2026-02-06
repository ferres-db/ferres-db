import { useParams, useNavigate } from 'react-router-dom';
import { useEffect } from 'react';

export const CollectionDetails = () => {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  
  useEffect(() => {
    if (name) {
      navigate(`/collections/${name}/points`, { replace: true });
    }
  }, [name, navigate]);

  return null;
};
