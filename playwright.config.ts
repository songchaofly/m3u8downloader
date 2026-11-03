import { defineConfig } from '@playwright/test';
export default defineConfig({testDir:'tests/ui',use:{channel:'msedge',baseURL:'http://127.0.0.1:1420',viewport:{width:1320,height:860},screenshot:'only-on-failure'},webServer:{command:'npm run dev',url:'http://127.0.0.1:1420',reuseExistingServer:true},reporter:[['list'],['html',{open:'never'}]]});
