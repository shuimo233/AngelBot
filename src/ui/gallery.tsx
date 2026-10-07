import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './tokens.css';
import './system.css';
import './gallery.css';
import { UiLibrary } from './UiLibrary';

createRoot(document.getElementById('root')!).render(<StrictMode><UiLibrary /></StrictMode>);
