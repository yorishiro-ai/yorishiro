-- Disposable test databases copy template1.
\connect template1
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

\connect postgres
CREATE ROLE yorishiro LOGIN PASSWORD 'yorishiro' NOSUPERUSER NOBYPASSRLS CREATEDB CREATEROLE;
CREATE DATABASE yorishiro OWNER yorishiro;
