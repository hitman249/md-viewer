import { mount } from 'svelte';
import App from './App.svelte';
import './styles.css';

// Монтируем корневой компонент в контейнер, занимающий всё окно приложения.
mount(App, { target: document.getElementById('app') });
